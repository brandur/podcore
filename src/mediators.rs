use errors::*;
use model;

use diesel::prelude::*;
use diesel::pg::PgConnection;
use futures::Stream;
use hyper;
use hyper::{Client, Uri};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::str;
use std::str::FromStr;
use tokio_core::reactor::Core;

pub struct DirectoryPodcastUpdater<'a> {
    pub client:      &'a Client<hyper::client::HttpConnector, hyper::Body>,
    pub conn:        &'a PgConnection,
    pub core:        &'a mut Core,
    pub dir_podcast: &'a mut model::DirectoryPodcast,
}

impl<'a> DirectoryPodcastUpdater<'a> {
    pub fn run(&mut self) -> Result<()> {
        self.conn
            .transaction::<_, Error, _>(|| self.run_inner())
            .chain_err(|| "Error in database transaction")
    }

    fn run_inner(&mut self) -> Result<()> {
        let raw_url = self.dir_podcast.feed_url.clone().unwrap();
        let feed_url = Uri::from_str(raw_url.as_str())
            .chain_err(|| format!("Error parsing feed URL: {}", raw_url))?;
        let res = self.core
            .run(self.client.get(feed_url))
            .chain_err(|| format!("Error fetching feed URL: {}", raw_url))?;
        println!("Response: {}", res.status());
        let body = self.core
            .run(res.body().concat2())
            .chain_err(|| format!("Error reading body from URL: {}", raw_url))?;

        Self::parse_feed(str::from_utf8(&*body).unwrap())?;

        self.dir_podcast.feed_url = None;
        self.dir_podcast
            .save_changes::<model::DirectoryPodcast>(&self.conn)
            .chain_err(|| "Error saving changes to directory podcast")?;
        Ok(())
    }

    fn parse_feed(data: &str) -> Result<()> {
        let mut reader = Reader::from_str(data);
        reader.trim_text(true);

        let mut in_title = false;
        let mut buf = Vec::new();
        let mut ns_buf = Vec::new();

        loop {
            match reader.read_namespaced_event(&mut buf, &mut ns_buf) {
                Ok((ref ns, Event::Start(ref e))) => {
                    match *ns {
                        Some(ref ns_content) => println!(
                            "ns = {:?} e = {:?}",
                            str::from_utf8(*ns_content).unwrap(),
                            str::from_utf8(e.name()).unwrap()
                        ),
                        None => {
                            println!("(no namespace) e = {:?}", str::from_utf8(e.name()).unwrap())
                        }
                    };
                    match e.name() {
                        b"title" => in_title = true,
                        _ => (),
                    }
                }
                Ok((ref _ns, Event::Text(ref e))) => {
                    if in_title {
                        println!("title = {}", e.unescape_and_decode(&reader).unwrap())
                    }
                }
                Ok((_, Event::End(ref e))) => match e.name() {
                    b"title" => in_title = false,
                    _ => (),
                },
                Ok((_, Event::Eof)) => break,
                Err(e) => bail!("Error at position {}: {:?}", reader.buffer_position(), e),
                _ => (),
            };
        }
        buf.clear();
        ns_buf.clear();

        Ok(())
    }
}

#[test]
fn test_run() {
    use diesel;
    use schema::directories_podcasts;
    use test_helpers;

    let conn = test_helpers::connection();
    let mut core = Core::new().unwrap();
    let client = Client::new(&core.handle());

    let itunes = model::Directory::itunes(&conn).unwrap();
    let mut dir_podcast = model::DirectoryPodcast {
        id:           0,
        directory_id: itunes.id,
        feed_url:     Some("http://feeds.feedburner.com/RoderickOnTheLine".to_owned()),
        podcast_id:   None,
        vendor_id:    "471418144".to_owned(),
    };
    diesel::insert_into(directories_podcasts::table)
        .values(&dir_podcast)
        .execute(&conn)
        .unwrap();

    let mut updater = DirectoryPodcastUpdater {
        client:      &client,
        conn:        &conn,
        core:        &mut core,
        dir_podcast: &mut dir_podcast,
    };
    updater.run().unwrap();
}

/*
struct PodcastUpdater {
    pub podcast: &Podcast,
}

impl PodcastUpdater {
    fn run(&self) {}
}
*/
