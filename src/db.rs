use std::collections::HashSet;
use std::path::{Path, PathBuf};
use itertools::Itertools;
use log::debug;
use rusqlite::Connection;

const FAVOURITES_DB: &str = "favourites.db";

#[derive(Debug)]
pub struct DB {
    pub connection: Connection,
    folder: PathBuf,
}

impl DB {
    pub fn new(folder: &PathBuf) -> Self {
        debug!("init new DB connection");
        let db_file_path = get_db_file(&folder);
        let connection = Connection::open(db_file_path).expect("cannot open DB connection");
        connection.execute(
            "create table if not exists favourites (path text primary key)",
            (),
        ).expect("cannot create table");
        let folder_out = folder.clone();

        Self {connection, folder: folder_out}
    }

    pub fn insert(&self, img_path: &PathBuf) {
        let record = self.prepare_record(img_path);
        debug!("insert {} to DB", record);
        self.connection.execute(
            "INSERT INTO favourites (path) values (?1)",
            [record],
        ).expect("cannot save record");
    }

    pub fn delete(&self, img_path: &PathBuf) {
        let record = self.prepare_record(img_path);
        debug!("delete {} from DB", record);
        self.connection.execute(
            "DELETE FROM favourites where path = (?1)",
            [record],
        ).expect("cannot delete record");
    }

    pub fn get_all(&self) -> HashSet<PathBuf> {
        debug!("run select * statement");
        let mut stmt = self.connection.prepare("SELECT path from favourites").expect("cannot prepare query");

        stmt
            .query_map((), |row| { Ok(row.get(0)?) })
            .expect("cannot get data")
            .map(|e| self.folder.join(self.join_path_parts(e.unwrap())))
            .filter(|file| file.exists())
            .collect()
    }

    pub fn close(&self) {
        debug!("close DB connection");
        self.connection.close().expect("cannot close DB connection");
    }

    fn prepare_record(&self, img_path: &PathBuf) -> String {
        img_path.strip_prefix(self.folder.as_path())
            .unwrap()
            .components()
            .map(|component| component.as_os_str().to_str().unwrap())
            .join("\t")
    }

    fn join_path_parts(&self, path_with_tabs: String) -> PathBuf {
        let mut path = PathBuf::new();

        for part in path_with_tabs.split("\t") {
            path.push(part);
        }

        path
    }
}

pub fn get_db_file(folder: &PathBuf) -> PathBuf {
    folder.join(Path::new(FAVOURITES_DB))
}
