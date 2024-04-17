use std::{fmt::Debug, process::exit};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Url;
use reqwest_middleware::ClientWithMiddleware;
use serde::de::DeserializeOwned;

use crate::{
    cli::ActionCommands,
    data_types::{
        request::ActionPayload,
        response::{Episode, Movie, PaginatedResponse, TVShow},
    },
};

pub struct Action {
    pub client: ClientWithMiddleware,
    pub base_url: Url,
    pub action: ActionCommands,
    pub offset: u32,
    pub limit: Option<u32>,
    pub pb: ProgressBar,
}

impl Action {
    pub fn new(client: ClientWithMiddleware, base_url: Url) -> Self {
        let pb = ProgressBar::new(0);
        Self {
            client,
            base_url,
            action: ActionCommands::Sync,
            offset: 0,
            limit: None,
            pb,
        }
    }

    async fn get_all<T>(
        &self,
        mut url: Url,
        limit_records: bool,
    ) -> Result<PaginatedResponse<T>, Box<dyn std::error::Error>>
    where
        T: DeserializeOwned,
        T: Debug,
    {
        if limit_records {
            let length: i32 = match self.limit {
                Some(val) => val as i32,
                None => -1,
            };
            url.query_pairs_mut()
                .append_pair("length", &length.to_string())
                .append_pair("start", &self.offset.to_string());
        }
        let req = self.client.get(url);
        let res = req.send().await?;
        let body: PaginatedResponse<T> = res.json().await?;
        Ok(body)
    }

    async fn perform(
        &self,
        payload: ActionPayload,
    ) -> Result<reqwest::Response, reqwest_middleware::Error> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("subtitles");
        let action_string: String = self.action.to_string();
        url.query_pairs_mut().append_pair("action", &action_string);
        self.client.patch(url).json(&payload).send().await
    }

    async fn process_episode_subtitle(&self, pb: &ProgressBar, episode: Episode) {
        for subtitle in episode.subtitles {
            if !subtitle.is_valid() {
                continue;
            }

            pb.set_message(format!(
                "Performing action {} on {} subtitle of episode {}",
                self.action.to_string(),
                subtitle.audio_language_item.name,
                episode.title,
            ));
            let payload = ActionPayload {
                id: episode.sonarr_episode_id,
                media_type: String::from("episode"),
                language: subtitle.audio_language_item.code2.unwrap(),
                path: subtitle.path.unwrap(),
            };
            match self.perform(payload).await {
                Ok(res) => match res.error_for_status() {
                    Ok(_) => {
                        pb.set_message(format!(
                            "Successfully performed action {} on {} subtitle of episode {}",
                            self.action.to_string(),
                            subtitle.audio_language_item.name,
                            episode.title,
                        ));
                    }
                    Err(err) => {
                        pb.set_message(format!(
                            "Error performing action {} on {} subtitle of episode {}: {}",
                            self.action.to_string(),
                            subtitle.audio_language_item.name,
                            episode.title,
                            err,
                        ));
                    }
                },
                Err(err) => {
                    self.pb
                        .set_message(format!("Error connecting to Bazarr: {}", err));
                    exit(1);
                }
            }
        }
    }

    async fn process_movie_subtitle(&self, movie: Movie) {
        for subtitle in movie.subtitles {
            if !subtitle.is_valid() {
                continue;
            }
            self.pb.set_message(format!(
                "Performing action {} on {} subtitle of movie {}",
                self.action.to_string(),
                subtitle.audio_language_item.name,
                movie.title,
            ));
            let payload = ActionPayload {
                id: movie.radarr_id,
                media_type: String::from("movie"),
                language: subtitle.audio_language_item.code2.unwrap(),
                path: subtitle.path.unwrap(),
            };
            match self.perform(payload).await {
                Ok(res) => match res.error_for_status() {
                    Ok(_) => {
                        self.pb.set_message(format!(
                            "Successfully performed action {} on {} subtitle of movie {}",
                            self.action.to_string(),
                            subtitle.audio_language_item.name,
                            movie.title,
                        ));
                    }
                    Err(err) => {
                        self.pb.set_message(format!(
                            "Error performing action {} on {} subtitle of episode {}: {}",
                            self.action.to_string(),
                            subtitle.audio_language_item.name,
                            movie.title,
                            err,
                        ));
                    }
                },
                Err(err) => {
                    self.pb
                        .set_message(format!("Error connecting to Bazarr: {}", err));
                    exit(1);
                }
            }
        }
    }

    pub async fn movies(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.pb.set_style(
            ProgressStyle::with_template("[{bar:60.green/yellow}] {pos:>7}/{len:7} Movies\n{msg}")
                .unwrap()
                .progress_chars("##-"),
        );
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("movies");
        let response = self.get_all::<Movie>(url, true).await?;
        let num_movies: u64 = response.data.len() as u64;
        if num_movies == 0 {
            self.pb.finish_with_message("No movies found");
            return Ok(());
        }

        self.pb.set_length(num_movies);
        for movie in response.data {
            self.process_movie_subtitle(movie).await;
            self.pb.inc(1);
        }
        self.pb.finish_with_message(format!(
            "Finished performing action {} on all movies",
            self.action.to_string(),
        ));
        Ok(())
    }

    pub async fn tv_shows(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mp = MultiProgress::new();
        let pb_main = mp.add(self.pb.clone());
        pb_main.set_style(
            ProgressStyle::with_template(
                "[{bar:60.green/yellow}] {pos:>7}/{len:7} TV Shows\n{msg}",
            )
            .unwrap()
            .progress_chars("##-"),
        );
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("series");
        let response = self.get_all::<TVShow>(url.clone(), true).await?;
        let num_series: u64 = response.data.len() as u64;
        if num_series == 0 {
            pb_main.finish_with_message("No tv shows found");
            return Ok(());
        }

        pb_main.set_length(num_series);
        let sub_pb = mp.insert_after(&pb_main, ProgressBar::new(0));
        sub_pb.set_style(
            ProgressStyle::with_template("[{bar:60.cyan/blue}] {pos:>7}/{len:7} Episodes\n{msg}")
                .unwrap()
                .progress_chars("##-"),
        );
        url.path_segments_mut().unwrap().pop().push("episodes");
        for series in response.data {
            pb_main.set_message(format!("Processing tv show {}", series.title,));
            let query_param = format!("seriesid[]={}", series.sonarr_series_id);
            let mut new_url = url.clone();
            new_url.set_query(Some(&query_param));
            let response = self.get_all::<Episode>(new_url, false).await?;
            let num_episodes: u64 = response.data.len() as u64;
            sub_pb.set_length(num_episodes);
            if num_episodes == 0 {
                sub_pb.finish_with_message("No episodes found");
                return Ok(());
            }
            for episode in response.data {
                self.process_episode_subtitle(&sub_pb, episode).await;
                sub_pb.inc(1);
            }
            pb_main.inc(1);
            pb_main.set_message(format!("Finished processing tv show {}", series.title,));
        }
        pb_main.finish_with_message(format!(
            "Finished performing action {} on all tv shows",
            self.action.to_string(),
        ));
        Ok(())
    }
}
