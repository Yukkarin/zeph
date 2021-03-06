extern crate hyper;

use self::hyper::client::Client;

use std::thread;
use std::time::Duration;

use super::{Image,process_downloads,req_and_parse};

use std::sync::mpsc::Receiver;

pub fn main(rc: &Receiver<()>) {
    let client = Client::new();
    let mut url_string = "https://derpibooru.org/search.json?q=score.gt:0&filter_id=56027".to_string();
    let mut page = 1;

    loop {
        let res = match req_and_parse(&client, &url_string) {
            Ok(x) => x,
            Err(_) => {
                thread::sleep(Duration::new(3,0));
                continue
            }
        };

        let images = res.as_object().unwrap()["search"].as_array().unwrap();
        if images.is_empty() { break }

        let images = images.iter().fold(Vec::new(), |mut acc, x| {
            let image = x.as_object().unwrap();
            let mut rating = String::new();

            let tags = image["tags"].as_str().unwrap().split(',').map(|x| x.trim().replace(" ", "_")).filter_map(|x| {
                if x.starts_with("artist:") {
                    Some(x.split(':').collect::<Vec<_>>()[1].to_string())
                } else if x == "safe" || x == "semi-grimdark" {
                    rating = "s".to_string();
                    None
                } else if x == "explicit" || x == "grimdark" || x == "grotesque" {
                    rating = "e".to_string();
                    None
                } else if x == "questionable" || x == "suggestive" {
                    rating = "q".to_string();
                    None
                } else {
                    Some(x.to_string())
                }}).collect::<Vec<_>>();
            let rating = rating.chars().collect::<Vec<_>>()[0];
            let url = format!("https:{}", image["image"].as_str().unwrap());
            let id = image["id"].as_str().unwrap().parse::<u64>().unwrap();

            let ext = image["file_name"].as_str().unwrap().split('.').collect::<Vec<_>>();
            let ext = ext.last().unwrap();
            let name = format!("derpibooru_{}.{}", id, ext);
            let score = image["score"].as_i64().unwrap();


            acc.push(Image{
                    name: name,
                    got_from: "derpi".to_string(),
                    url: url,
                    tags: tags,
                    rating: rating,
                    post_url: format!("https://derpibooru.org/{}", id),
                    score: score as i32
                });
            acc
        });

        if process_downloads(&client, &images, rc).is_err() { break }

        page += 1;

        url_string = format!("https://derpibooru.org/search.json?q=score.gt:0&filter_id=56027&page={}", page);
    }
}
