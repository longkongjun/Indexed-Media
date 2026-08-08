use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(super) struct SearchPageDto {
    pub page: Option<u16>,
    pub total_pages: Option<u16>,
    #[serde(default)]
    pub results: Vec<WorkDto>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct FindResponseDto {
    #[serde(default)]
    pub movie_results: Vec<WorkDto>,
    #[serde(default)]
    pub tv_results: Vec<WorkDto>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct WorkDto {
    pub id: i64,
    pub title: Option<String>,
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub original_name: Option<String>,
    pub original_language: Option<String>,
    pub release_date: Option<String>,
    pub first_air_date: Option<String>,
    pub overview: Option<String>,
}

impl WorkDto {
    pub fn original_language(&self) -> Option<&str> {
        self.original_language.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct EpisodeDto {
    pub id: i64,
    pub season_number: u16,
    pub episode_number: u16,
    pub name: String,
}
