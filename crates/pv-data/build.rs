use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

const GEONAMES_BASE_URL: &str = "https://download.geonames.org/export/dump";
const GEONAMES_CITIES_FILE: &str = "cities1000.zip";
const GEONAMES_CITIES_TXT: &str = "cities1000.txt";
const CATALOG_FILE: &str = "geonames_city_catalog.bin.zst";
const CATALOG_VARIANT: &str = "ranked_aliases_10";
const ALIAS_CAP: usize = 10;

fn main() {
    println!("cargo:rerun-if-env-changed=PV_DATA_GEONAMES_ZIP");
    println!("cargo:rerun-if-env-changed=PV_DATA_GEONAMES_URL");

    if let Err(error) = build_city_catalog() {
        panic!("failed to build embedded GeoNames city catalog: {error}");
    }
}

fn build_city_catalog() -> Result<(), Box<dyn Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let catalog_path = out_dir.join(CATALOG_FILE);
    let zip_path = geonames_zip_path(&out_dir)?;
    if output_is_fresh(&catalog_path, &zip_path)? {
        return Ok(());
    }

    println!(
        "cargo:warning=building embedded GeoNames city catalog from {}",
        zip_path.display()
    );
    let cities = load_geonames_cities(&zip_path)?;
    if cities.is_empty() {
        return Err("GeoNames catalog is empty".into());
    }
    let encoded = encode_city_catalog(&cities)?;
    let compressed = zstd::stream::encode_all(encoded.as_slice(), 19)?;
    fs::write(&catalog_path, compressed)?;
    println!(
        "cargo:rustc-env=PV_DATA_GEONAMES_CITY_COUNT={}",
        cities.len()
    );
    Ok(())
}

fn geonames_zip_path(out_dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if let Ok(path) = env::var("PV_DATA_GEONAMES_ZIP") {
        let path = PathBuf::from(path);
        if !path.exists() {
            return Err(format!("PV_DATA_GEONAMES_ZIP does not exist: {}", path.display()).into());
        }
        println!("cargo:rerun-if-changed={}", path.display());
        return Ok(path);
    }

    if let Some(path) = workspace_artifact_zip()? {
        println!("cargo:rerun-if-changed={}", path.display());
        return Ok(path);
    }

    let raw_dir = out_dir.join("geonames_raw");
    fs::create_dir_all(&raw_dir)?;
    let zip_path = raw_dir.join(GEONAMES_CITIES_FILE);
    if !zip_path.exists() {
        download_geonames_zip(&zip_path)?;
    }
    Ok(zip_path)
}

fn workspace_artifact_zip() -> Result<Option<PathBuf>, Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let Some(workspace_root) = manifest_dir.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    let path = workspace_root
        .join("artifacts")
        .join("geonames")
        .join("raw")
        .join(GEONAMES_CITIES_FILE);
    Ok(path.exists().then_some(path))
}

fn output_is_fresh(output: &Path, input: &Path) -> Result<bool, Box<dyn Error>> {
    if !output.exists() {
        return Ok(false);
    }
    let output_modified = output.metadata()?.modified()?;
    let input_modified = input.metadata()?.modified()?;
    Ok(output_modified >= input_modified)
}

fn download_geonames_zip(zip_path: &Path) -> Result<(), Box<dyn Error>> {
    let url = env::var("PV_DATA_GEONAMES_URL")
        .unwrap_or_else(|_| format!("{GEONAMES_BASE_URL}/{GEONAMES_CITIES_FILE}"));
    let tmp_path = zip_path.with_extension("zip.tmp");
    println!("cargo:warning=downloading {url}");
    let status = Command::new("curl")
        .arg("--fail")
        .arg("--location")
        .arg("--show-error")
        .arg("--output")
        .arg(&tmp_path)
        .arg(&url)
        .status()?;
    if !status.success() {
        return Err(format!("curl exited with status {status}").into());
    }
    fs::rename(tmp_path, zip_path)?;
    Ok(())
}

#[derive(Debug, Clone)]
struct GeonamesCity {
    geoname_id: u32,
    name: String,
    ascii_name: String,
    alternate_names: String,
    latitude: f64,
    longitude: f64,
    feature_code: String,
    country_code: String,
    population: u64,
}

fn load_geonames_cities(zip_path: &Path) -> Result<Vec<GeonamesCity>, Box<dyn Error>> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let zip_file = archive.by_name(GEONAMES_CITIES_TXT)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, zip_file);
    let mut line = String::new();
    let mut cities = Vec::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if let Some(city) = parse_geonames_city(line.trim_end())? {
            cities.push(city);
        }
    }
    Ok(cities)
}

fn parse_geonames_city(line: &str) -> Result<Option<GeonamesCity>, Box<dyn Error>> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() < 19 {
        return Err(format!("invalid GeoNames city row with {} fields", fields.len()).into());
    }
    Ok(Some(GeonamesCity {
        geoname_id: fields[0].parse()?,
        name: fields[1].to_string(),
        ascii_name: fields[2].to_string(),
        alternate_names: fields[3].to_string(),
        latitude: fields[4].parse()?,
        longitude: fields[5].parse()?,
        feature_code: fields[7].to_string(),
        country_code: fields[8].to_string(),
        population: fields[14].parse().unwrap_or(0),
    }))
}

fn encode_city_catalog(cities: &[GeonamesCity]) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::with_capacity(cities.len() * 96);
    out.extend_from_slice(b"PVCITYCAT1\n");
    out.extend_from_slice(CATALOG_VARIANT.as_bytes());
    out.push(0);
    out.push(1);
    out.push(1);
    write_u16(ALIAS_CAP.try_into()?, &mut out);
    write_u32(cities.len().try_into()?, &mut out);

    for city in cities {
        write_u32(city.geoname_id, &mut out);
        write_string(&city.name, &mut out)?;
        write_string(&city.ascii_name, &mut out)?;
        write_i32((city.latitude * 1_000_000.0).round() as i32, &mut out);
        write_i32((city.longitude * 1_000_000.0).round() as i32, &mut out);
        write_string(&city.country_code, &mut out)?;
        write_u32(city.population.min(u64::from(u32::MAX)) as u32, &mut out);
        write_string(&city.feature_code, &mut out)?;
        let aliases = prune_city_aliases(city);
        write_u16(aliases.len().try_into()?, &mut out);
        for alias in aliases {
            write_string(&alias, &mut out)?;
        }
    }
    Ok(out)
}

fn write_string(value: &str, out: &mut Vec<u8>) -> Result<(), Box<dyn Error>> {
    let bytes = value.as_bytes();
    let len: u16 = bytes
        .len()
        .try_into()
        .map_err(|_| format!("string is too long for catalog field: {value}"))?;
    write_u16(len, out);
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_u16(value: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(value: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i32(value: i32, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn prune_city_aliases(city: &GeonamesCity) -> Vec<String> {
    if city.alternate_names.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    seen.insert(normalize_search_text(&city.name));
    seen.insert(normalize_search_text(&city.ascii_name));

    let mut aliases = city
        .alternate_names
        .split(',')
        .filter_map(|alias| {
            let alias = alias.trim();
            if !is_useful_city_alias(alias) {
                return None;
            }
            let normalized = normalize_search_text(alias);
            if normalized.is_empty() || !seen.insert(normalized) {
                return None;
            }
            Some(alias.to_string())
        })
        .collect::<Vec<_>>();
    aliases.sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
    aliases.truncate(ALIAS_CAP);
    aliases
}

fn is_useful_city_alias(alias: &str) -> bool {
    let chars = alias.chars().count();
    (2..=48).contains(&chars)
        && !alias.starts_with("http")
        && alias.chars().any(char::is_alphabetic)
        && alias
            .chars()
            .all(|char| !char.is_control() && char != '\t' && char != ',')
}

fn normalize_search_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|char| char.is_alphanumeric())
        .collect()
}
