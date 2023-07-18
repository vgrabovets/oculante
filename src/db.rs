use std::path::PathBuf;
use itertools::Itertools;
use rusqlite::Connection;

#[derive(Debug)]
pub struct DB {
    connection: Connection,
    image_folder: PathBuf,
}

impl DB {
    pub fn new(db_file: PathBuf, image_folder: PathBuf) -> Self {
        let db_file_exists = db_file.exists();
        let connection = Connection::open(db_file).expect("cannot open DB connection");
        if !db_file_exists {
            connection.execute(
                "create table if not exists favourites (path text primary key)",
                (),
            ).expect("cannot create table");
        }
        Self {connection, image_folder}
    }

    pub fn insert(&self, img_path: &PathBuf) {
        let record = self.prepare_record(img_path);
        self.connection.execute(
            "INSERT INTO favourites (path) values (?1)",
            [record],
        ).expect("cannot save record");
    }

    pub fn delete(&self, img_path: &PathBuf) {
        let record = self.prepare_record(img_path);
        self.connection.execute(
            "DELETE FROM favourites where path = (?1)",
            [record],
        ).expect("cannot delete record");
    }

    fn prepare_record(&self, img_path: &PathBuf) -> String {
        img_path.strip_prefix(self.image_folder.as_path())
            .unwrap()
            .components()
            .map(|component| component.as_os_str().to_str().unwrap())
            .join("\t")
    }
}
