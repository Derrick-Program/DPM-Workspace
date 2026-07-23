#![allow(warnings)]
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;
use walkdir::WalkDir;
use zip::ZipArchive;
use zip::{write::FileOptions, CompressionMethod, ZipWriter};

pub fn zip_folder(folder_path: &Path, zip_file_path: &Path) -> io::Result<()> {
    let file = File::create(zip_file_path)?;
    let walkdir = WalkDir::new(folder_path);
    let it = walkdir.into_iter();

    let mut zip = ZipWriter::new(file);
    let options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated) // 使用Deflated压缩
        .unix_permissions(0o755);

    for entry in it.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = path.strip_prefix(Path::new(folder_path)).unwrap();

        if path.is_file() {
            zip.start_file(name.to_string_lossy(), options)?;
            let mut f = File::open(path)?;
            io::copy(&mut f, &mut zip)?;
        } else if !name.as_os_str().is_empty() {
            // 确保目录以 / 结尾
            zip.add_directory(name.to_string_lossy() + "/", options)?;
        }
    }
    zip.finish()?;
    Ok(())
}

pub fn unzip_file(zip_file_path: &Path, output_folder: &Path, name: &str) -> io::Result<()> {
    let zip_file = File::open(zip_file_path)?;
    let mut archive = ZipArchive::new(BufReader::new(zip_file))?;
    let output_folder = output_folder.join(name);
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let output_path = match file.enclosed_name() {
            Some(path) => output_folder.join(path),
            None => continue,
        };

        if (*file.name()).ends_with('/') {
            std::fs::create_dir_all(&output_path)?;
        } else {
            if let Some(p) = output_path.parent() {
                if !p.exists() {
                    std::fs::create_dir_all(&p)?;
                }
            }
            let mut outfile = File::create(&output_path)?;
            io::copy(&mut file, &mut outfile)?;
        }
    }

    Ok(())
}

pub fn read_file_from_zip(zip_file_path: &Path, file_name: &str) -> io::Result<String> {
    let zip_file = File::open(zip_file_path)?;
    let mut archive = ZipArchive::new(BufReader::new(zip_file))?;
    let mut file = match archive.by_name(file_name) {
        Ok(file) => file,
        Err(_) => {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "File not found in zip",
            ))
        }
    };
    let mut content = String::new();
    file.read_to_string(&mut content)?;

    Ok(content)
}
