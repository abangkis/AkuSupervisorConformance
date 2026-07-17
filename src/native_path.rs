use std::path::PathBuf;

pub fn normalize(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    let text = path.as_os_str().to_string_lossy();
    if let Some(remainder) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{remainder}"))
    } else if let Some(remainder) = text.strip_prefix(r"\\?\") {
        PathBuf::from(remainder)
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;
    use std::path::PathBuf;

    #[test]
    fn windows_verbatim_paths_are_safe_for_external_runtimes() {
        if cfg!(windows) {
            assert_eq!(
                normalize(PathBuf::from(r"\\?\C:\workspace\fixture")),
                PathBuf::from(r"C:\workspace\fixture")
            );
        }
    }
}
