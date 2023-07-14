use crate::utils::is_ext_compatible;
use anyhow::{bail, Context, Result};
use log::debug;
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::default::Default;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Default)]
pub struct Scrubber {
    pub index: usize,
    pub entries: Vec<PathBuf>,
    pub wrap: bool,
    pub favourites: HashSet<PathBuf>,
}

impl Scrubber {
    pub fn new(
        path: &Path,
        favourites_file: Option<&str>,
        randomize: bool,
        walk_files: bool,
        intersperse_with_favs_every_n: Option<usize>,
    ) -> Self {
        let (entries, favourites) = get_image_filenames_for_directory(
            path,
            favourites_file,
            randomize,
            walk_files,
            intersperse_with_favs_every_n,
        )
            .unwrap_or_default();
        let index = entries.iter().position(|p| p == path).unwrap_or_default();
        Self {
            index,
            entries,
            wrap: true,
            favourites,
        }
    }
    pub fn next(&mut self) -> PathBuf {
        self.index += 1;
        if self.index > self.entries.len().saturating_sub(1) {
            if self.wrap {
                self.index = 0;
            } else {
                self.index = self.entries.len().saturating_sub(1);
            }
        }
        // debug!("{:?}", self.entries.get(self.index));
        self.entries.get(self.index).cloned().unwrap_or_default()
    }

    pub fn prev(&mut self) -> PathBuf {
        if self.index == 0 {
            if self.wrap {
                self.index = self.entries.len().saturating_sub(1);
            }
        } else {
            self.index = self.index.saturating_sub(1);
        }
        // debug!("{:?}", self.entries.get(self.index));
        self.entries.get(self.index).cloned().unwrap_or_default()
    }

    pub fn set(&mut self, index: usize) -> PathBuf {
        if index < self.entries.len() {
            self.index = index;
        }
        debug!("{:?}", self.entries.get(self.index));
        self.entries.get(self.index).cloned().unwrap_or_default()
    }

    pub fn len(&mut self) -> usize {
        self.entries.len()
    }
}

// Get sorted list of files in a folder
// TODO: Should probably return an Result<T,E> instead, but am too lazy to figure out + handle a dedicated error type here
// TODO: Cache this result, instead of doing it each time we need to fetch another file from the folder
pub fn get_image_filenames_for_directory(
    folder_path: &Path,
    favourites_file: Option<&str>,
    randomize: bool,
    walk_files: bool,
    intersperse_with_favs_every_n: Option<usize>,
) -> Result<(Vec<PathBuf>, HashSet<PathBuf>)> {
    let mut folder_path = folder_path.to_path_buf();
    if folder_path.is_file() {
        folder_path = folder_path
            .parent()
            .map(|p| p.to_path_buf())
            .context("Can't get parent")?;
    }

    let mut favourites: HashSet<PathBuf> = Default::default();

    if let Some(favourites_file) = favourites_file {
        let favourites_path = folder_path.join(Path::new(favourites_file));
        if favourites_path.exists() {
            let file = std::fs::File::open(favourites_path)?;
            let reader = BufReader::new(file);
            favourites = reader
                .lines()
                .filter_map(|line| line.ok())
                .map(|file_str| folder_path.join(join_path_parts(file_str)))
                .filter(|file| file.exists())
                .collect();
        }
    }

    let mut dir_files: Vec<PathBuf>;

    if walk_files {
        dir_files = WalkDir::new(folder_path)
            .into_iter()
            .filter_map(|v| v.ok())
            .map(|entry| entry.into_path())
            .filter(|x| is_ext_compatible(x))
            .collect::<Vec<PathBuf>>();
    } else {
        let info = std::fs::read_dir(folder_path)?;
        dir_files = info
            .flat_map(|x| x)
            .map(|x| x.path())
            .filter(|x| is_ext_compatible(x))
            .collect::<Vec<PathBuf>>();
    }

    // TODO: Are symlinks handled correctly?

    let mut favourites_vec: Vec<PathBuf> = favourites.clone().into_iter().collect();

    if randomize {
        let mut rng = rand::thread_rng();
        dir_files.shuffle(&mut rng);
        favourites_vec.shuffle(&mut rng);
    } else {
        dir_files.sort_unstable_by(|a, b| {
            lexical_sort::natural_lexical_cmp(
                &a.file_name()
                    .map(|f| f.to_string_lossy())
                    .unwrap_or_default(),
                &b.file_name()
                    .map(|f| f.to_string_lossy())
                    .unwrap_or_default(),
            )
        });
    }

    if let Some(every_n) = intersperse_with_favs_every_n {
        dir_files = insert_after_every(dir_files, favourites_vec, every_n);
    }
    debug!("number of files: {}", dir_files.len());
    return Ok((dir_files, favourites));
}

/// Find first valid image from the directory
/// Assumes the given path is a directory and not a file
pub fn find_first_image_in_directory(folder_path: &PathBuf) -> Result<PathBuf> {
    if !folder_path.is_dir() {
        bail!("This is not a folder");
    };
    get_image_filenames_for_directory(
        folder_path,
        None,
        false,
        false,
        None,
    )
        .map(|(x, _)| {
        x.first()
            .cloned()
            .context("Folder does not have any supported images in it")
    })?
}

fn join_path_parts(path_with_tabs: String) -> PathBuf {
    let mut path = PathBuf::new();

    for part in path_with_tabs.split("\t") {
        path.push(part);
    }

    path
}

fn insert_after_every(main_vector: Vec<PathBuf>, other_vector: Vec<PathBuf>, after: usize) -> Vec<PathBuf> {
    let mut result = Vec::with_capacity(main_vector.len() + other_vector.len());
    let mut other_vector_i = 0;
    let other_vector_set: HashSet<PathBuf> = other_vector.clone().into_iter().collect();

    for (i, element) in main_vector.into_iter().enumerate() {
        if other_vector_set.contains(&element) {
            continue
        }

        result.push(element);
        if other_vector_i < other_vector.len() && (i + 1) % after == 0 {
            result.push(other_vector[other_vector_i].clone());
            other_vector_i += 1;
        }
    }

    while other_vector_i < other_vector.len() {
        result.push(other_vector[other_vector_i].clone());
        other_vector_i += 1;
    }

    result
}
