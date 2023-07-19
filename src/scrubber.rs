use crate::utils::is_ext_compatible;
use anyhow::{bail, Context, Result};
use log::debug;
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::default::Default;
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
        randomize: bool,
        walk_files: bool,
        favourites: Option<HashSet<PathBuf>>,
        intersperse_with_favs_every_n: usize,
    ) -> Self {
        let entries = get_image_filenames_for_directory(
            path,
            randomize,
            walk_files,
            &favourites,
            intersperse_with_favs_every_n,
        )
            .unwrap_or_default();
        let index = entries.iter().position(|p| p == path).unwrap_or_default();

        let favourites_out: HashSet<PathBuf>;

        if favourites.is_some() {
            favourites_out = favourites.unwrap();
        } else {
            favourites_out = Default::default();
        }

        Self {
            index,
            entries,
            wrap: true,
            favourites: favourites_out,
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

    pub fn get(&mut self, index: usize) -> Option<PathBuf> {
        self.entries.get(index).cloned()
    }

    pub fn len(&mut self) -> usize {
        self.entries.len()
    }

    pub fn re_initialize(&mut self, intersperse_with_favs_every_n: usize) {
        let entries_wo_favourites: Vec<PathBuf> = self.entries
            .iter()
            .filter(|element| !self.favourites.contains(*element))
            .map(|element| element.clone())
            .collect();

        let favourites_vec: Vec<PathBuf> = self.favourites.clone().into_iter().collect();
        self.entries = insert_after_every(entries_wo_favourites, favourites_vec, intersperse_with_favs_every_n);
    }
}

// Get sorted list of files in a folder
// TODO: Should probably return an Result<T,E> instead, but am too lazy to figure out + handle a dedicated error type here
// TODO: Cache this result, instead of doing it each time we need to fetch another file from the folder
pub fn get_image_filenames_for_directory(
    folder_path: &Path,
    randomize: bool,
    walk_files: bool,
    favourites: &Option<HashSet<PathBuf>>,
    intersperse_with_favs_every_n: usize,
) -> Result<Vec<PathBuf>> {
    let mut folder_path = folder_path.to_path_buf();
    if folder_path.is_file() {
        folder_path = folder_path
            .parent()
            .map(|p| p.to_path_buf())
            .context("Can't get parent")?;
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

    let mut favourites_vec: Vec<PathBuf> = favourites.clone().unwrap_or_default().into_iter().collect();

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

    if intersperse_with_favs_every_n > 0 {
        dir_files = insert_after_every(dir_files, favourites_vec, intersperse_with_favs_every_n);
    }
    return Ok(dir_files);
}

/// Find first valid image from the directory
/// Assumes the given path is a directory and not a file
pub fn find_first_image_in_directory(folder_path: &PathBuf) -> Result<PathBuf> {
    if !folder_path.is_dir() {
        bail!("This is not a folder");
    };
    get_image_filenames_for_directory(
        folder_path,
        false,
        false,
        &None,
        0,
    )
        .map(|x| {
        x.first()
            .cloned()
            .context("Folder does not have any supported images in it")
    })?
}

fn insert_after_every(main_vector: Vec<PathBuf>, other_vector: Vec<PathBuf>, after: usize) -> Vec<PathBuf> {
    let mut result = Vec::with_capacity(main_vector.len());
    let mut i = 0;
    let mut other_vector_i = 0;
    let other_vector_set: HashSet<PathBuf> = other_vector.clone().into_iter().collect();

    for element in main_vector.into_iter() {
        if other_vector_set.contains(&element) {
            continue
        }

        result.push(element);
        i += 1;

        if other_vector_i < other_vector.len() && i % after == 0 {
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
