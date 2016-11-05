#![cfg(feature = "postgresql")]

extern crate postgres;
extern crate crypto;

use self::crypto::scrypt::{scrypt_simple,scrypt_check,ScryptParams};

use self::postgres::{Connection, TlsMode, Result as SQLResult};
use self::postgres::rows::Row;

pub struct Db(Connection);

use super::{Image,Tag,AnyWith,ImageBuilder,parse_tag};

impl Default for Db { // Чтобы Clippy не жаловался
    fn default() -> Self {
        Self::new()
    }
}

static POSTGRES_LOGIN : &'static str = "easy";
static POSTGRES_PASS : &'static str = "";

lazy_static! {
    static ref SCRYPT_PARAMS: ScryptParams = ScryptParams::new(10, 8, 1); // 10 сильно быстрее чем 14
}

impl Db {
    pub fn new() -> Self {
        let conn = Connection::connect(format!("postgres://{}:{}@localhost", POSTGRES_LOGIN, POSTGRES_PASS), TlsMode::None).unwrap();
        conn.batch_execute("CREATE EXTENSION IF NOT EXISTS citext;
                            CREATE TABLE IF NOT EXISTS images(
                                id SERIAL PRIMARY KEY,
                                name TEXT NOT NULL UNIQUE,
                                tags TEXT[] NOT NULL,
                                uploader TEXT,
                                score INT NOT NULL DEFAULT 0,

                                got_from TEXT,
                                original_link TEXT,
                                rating CHAR
                            );

                            CREATE TABLE IF NOT EXISTS users(
                                id SERIAL PRIMARY KEY,
                                name CITEXT UNIQUE NOT NULL,
                                pass TEXT NOT NULL
                            );").unwrap();
        Db(conn)
    }

    /// Сохранить картинку, сгенерировава имя из тэгов
    pub fn add_with_tags_name(&self, tags: &[String], ext: &str, uploader: &str) -> SQLResult<String> {
        let lastnum = self.0.query("SELECT id FROM images ORDER BY id DESC LIMIT 1", &[])?.get(0).get::<_, i32>("id");

        let name = format!("{}_{}.{}", lastnum + 1  , tags.join("_").replace("'","''"),ext);
        self.add_image(&ImageBuilder::new(&name, tags).uploader(uploader).finalize())?;
        Ok(name)
    }

    pub fn add_image(&self, image: &ImageBuilder) -> SQLResult<()> {
        self.0.execute("INSERT into images (name,
                                            tags,
                                            got_from,
                                            original_link,
                                            rating,
                                            uploader,
                                            score)
                       VALUES ($1,
                               $2,
                               $3,
                               $4,
                               $5,
                               $6,
                               $7) ON CONFLICT (name) DO UPDATE SET tags = $2, score = $7",
        &[&image.name,&image.tags,&image.got_from, &image.original_link,&image.rating.map(|x| x.to_string()), &image.uploader, &image.score]).unwrap();
        Ok(())
    }

    pub fn get_image(&self, id: i32) -> SQLResult<Option<Image>> {
        let rows = self.0.query("SELECT * FROM images WHERE id = $1", &[&id])?;
        Ok(if !rows.is_empty() {
            Some(Db::extract_image(rows.get(0)))
        } else {
            None
        })
    }

    pub fn get_images<T: Into<Option<i32>>>(&self, take: T, skip: usize) -> SQLResult<Vec<Image>>{
        let take = match take.into() {
            Some(x) => x.to_string(),
            None    => "ALL".to_string()
        };

        Ok(self.0.query(&format!("SELECT * FROM images ORDER BY id DESC LIMIT {} OFFSET {}", take, skip as i32),&[])?
           .iter().fold(Vec::new(), |mut acc, row| {
               acc.push(Db::extract_image(row));
               acc
           }))
    }

    pub fn by_tags<T: Into<Option<i32>>>(&self, take: T, skip: usize, tags: &[String]) -> SQLResult<Vec<Image>> {
        let tags = tags.iter().map(|x| parse_tag(x)).collect::<Vec<_>>();
        let order = tags.iter().filter_map(|x| {
            match *x {
                Tag::OrderBy(ref by, ref ascdesc) => {
                    Some(format!("{:?}", by).to_lowercase() + " " + &format!("{:?}", ascdesc).to_uppercase())
                },
                _   => None
            }
        }).collect::<Vec<_>>();
        let order = match order.last() {
            Some(t) => t,
            None    => "id DESC"
        };

        let q = tags.clone().iter().map(|t| {
            match *t {
                Tag::Include(ref incl) => format!(r"tags @> ARRAY['{}']", incl),
                Tag::Exclude(ref excl) => format!(r"NOT tags @> ARRAY['{}']", excl),
                Tag::AnyWith(ref x) => match *x {
                    AnyWith::Before(ref bef) => format!(r"(SELECT bool_or(tag ~ '^{}') FROM unnest(tags) t (tag))", bef),
                    AnyWith::After(ref aft) => format!(r"(SELECT bool_or(tag ~ '{}$') FROM unnest(tags) t (tag))", aft),
                },
                Tag::Rating(ref r) => {
                    let mut s = "(".to_string();
                    for tg in r {
                        s.push_str(&format!("rating = '{}' OR ", tg))
                    }
                    let _ = (0..4).inspect(|_| {s.pop(); }).collect::<Vec<_>>();
                    s.push_str(")");

                    s
                },

                Tag::From(ref f) => {
                    let mut s = "(".to_string();
                    for tg in f {
                        s.push_str(&format!("got_from = '{}' OR ", tg))
                    }
                    let _ = (0..4).inspect(|_| {s.pop(); }).collect::<Vec<_>>();
                    s.push_str(")");

                    s
                },

                Tag::Uploader(ref u) => {
                    let mut s = "(".to_string();
                    for tg in u {
                        s.push_str(&format!("uploader = '{}' OR ", tg))
                    }
                    let _ = (0..4).inspect(|_| {s.pop(); }).collect::<Vec<_>>();
                    s.push_str(")");

                    s
                },
                Tag::OrderBy(_,_) => {String::new()}
            }
        }).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" AND ");
        let q = if !q.is_empty() {
            format!("WHERE {}", q)
        } else {
            String::new()
        };

        let take = match take.into() {
            Some(x) => x.to_string(),
            None    => "ALL".to_string()
        };

        Ok(self.0.query(&format!("SELECT * FROM images {} ORDER BY {} LIMIT {} OFFSET {}", q, order, take, skip),&[])?
           .iter().fold(Vec::new(), |mut acc, row| {
               acc.push(Db::extract_image(row));
               acc
           }))
    }

    pub fn delete_image(&self, id: i32) -> SQLResult<String> {
        let name = self.0.query("SELECT * FROM images WHERE id = $1", &[&id])?.get(0).get::<_,String>("name");
        self.0.execute("DELETE FROM images WHERE id = $1", &[&id])?;
        Ok(name)
    }

    // true - всё хорошо, false - пользователь уже существует
    pub fn add_user(&self, login: &str, pass: &str) -> SQLResult<bool> {
        if self.0.query("SELECT * FROM users WHERE name = $1", &[&login])?.is_empty() && login.to_lowercase() != "sync" {
            let pass = scrypt_simple(pass, &SCRYPT_PARAMS).unwrap();

            self.0.execute("INSERT INTO users (name,pass) VALUES ($1,$2)", &[&login, &pass])?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    ///Result показывает ошибки в базе, Option - существует пользователь или нет
    pub fn check_user(&self, login: &str, pass: &str) -> SQLResult<Option<bool>> {
        let pass_hash = self.0.query("SELECT * FROM USERS WHERE name = $1", &[&login])?;
        if pass_hash.is_empty() {
            Ok(None)
        } else {
            let pass_hash = pass_hash.get(0).get::<_, String>("pass");
            Ok(Some(scrypt_check(pass, &pass_hash).unwrap()))
        }
    }

    fn extract_image(row: Row) -> Image {
        Image{
            id: row.get("id"),
            name: row.get("name"),
            tags: row.get("tags"),
            got_from: row.get::<_, Option<String>>("got_from"),
            original_link: row.get::<_, Option<String>>("original_link"),
            rating: row.get::<_,Option<String>>("rating").map(|x| x.to_string().chars().collect::<Vec<_>>()[0]),
            uploader: row.get::<_,Option<String>>("uploader"),
            score: row.get::<_,i32>("score")
        }
    }
}
