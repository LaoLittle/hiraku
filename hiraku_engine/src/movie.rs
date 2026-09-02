use std::collections::BTreeMap;

use bevy::prelude::Resource;
use hiraku_script::hson;
use serde::Deserialize;
use thiserror::Error;

use crate::vfs::{HdpVfs, VfsError};

#[derive(Clone, Debug, Default, Resource)]
pub struct MovieCatalog {
    movies: BTreeMap<String, MovieDefinition>,
}

#[derive(Clone, Debug)]
pub struct MovieDefinition {
    pub path: String,
}

impl MovieCatalog {
    pub fn resolve(&self, name: &str) -> Option<&MovieDefinition> {
        self.movies.get(name)
    }
}

#[derive(Debug, Deserialize)]
struct MovieFile {
    name: String,
    video: String,
}

#[derive(Debug, Error)]
pub enum MovieCatalogError {
    #[error("failed to read movie data: {0}")]
    Read(#[from] VfsError),
    #[error("failed to load movie data `{path}`: {message}")]
    Data { path: String, message: String },
}

pub fn load_movie_catalog(vfs: &HdpVfs) -> Result<MovieCatalog, MovieCatalogError> {
    let directory = vfs.load_movies_dir_path(None)?;
    let mut descriptor_paths = match vfs.list_files_recursive(&directory) {
        Ok(paths) => paths,
        Err(VfsError::NotFound(_)) => return Ok(MovieCatalog::default()),
        Err(error) => return Err(error.into()),
    };
    descriptor_paths.retain(|path| path.ends_with(".movie.hson"));
    descriptor_paths.sort();

    let mut movies = BTreeMap::new();
    for descriptor_path in descriptor_paths {
        let source = vfs.read_text(&descriptor_path)?;
        let file: MovieFile = hson::from_str(&source).map_err(|error| MovieCatalogError::Data {
            path: descriptor_path.clone(),
            message: error.render_with_options(
                &descriptor_path,
                &source,
                hiraku_script::RenderOptions::terminal(),
            ),
        })?;
        if file.name.trim().is_empty() {
            return Err(MovieCatalogError::Data {
                path: descriptor_path,
                message: "movie name must not be empty".into(),
            });
        }
        let lower = file.video.to_ascii_lowercase();
        if !lower.ends_with(".mkv") && !lower.ends_with(".webm") {
            return Err(MovieCatalogError::Data {
                path: descriptor_path,
                message: "movie video must be a `.mkv` or `.webm` AV1 + Opus asset".into(),
            });
        }
        let definition = MovieDefinition {
            path: vfs.resolve_path(Some(&descriptor_path), &file.video),
        };
        if movies.insert(file.name.clone(), definition).is_some() {
            return Err(MovieCatalogError::Data {
                path: descriptor_path,
                message: format!("movie `{}` is defined more than once", file.name),
            });
        }
    }
    Ok(MovieCatalog { movies })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_named_movie_descriptors() {
        let root =
            std::env::temp_dir().join(format!("hiraku-movie-catalog-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("movies")).expect("test directory must be created");
        std::fs::write(root.join("settings.hson"), ".{}").expect("test settings must be written");
        std::fs::write(
            root.join("movies/intro.movie.hson"),
            ".{ name: \"intro\", video: \"intro.webm\" }",
        )
        .expect("test descriptor must be written");

        let vfs = HdpVfs::new_with_config(&root, "settings.hson", "startup.hks");
        let catalog = load_movie_catalog(&vfs).expect("movie catalog must load");
        assert_eq!(
            catalog
                .resolve("intro")
                .expect("named movie must resolve")
                .path,
            "movies/intro.webm"
        );

        std::fs::remove_dir_all(root).expect("test directory must be removed");
    }
}
