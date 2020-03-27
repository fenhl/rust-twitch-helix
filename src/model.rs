//! Data types returned by the API

use {
    std::{
        collections::HashSet,
        fmt
    },
    chrono::{
        Duration,
        prelude::*
    },
    itertools::Itertools as _,
    reqwest::Url,
    serde::Deserialize,
    serde_json::Value as Json,
    crate::{
        Client,
        Error,
        HELIX_BASE_URL,
        paginated
    }
};

macro_rules! id_types {
    ($(#[$doc:meta] $T:ident),+) => {
        $(
            #[$doc]
            #[derive(Deserialize, Clone, PartialEq, Eq, Hash)]
            pub struct $T(pub String);

            impl fmt::Display for $T {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    self.0.fmt(f)
                }
            }

            impl AsRef<str> for $T {
                fn as_ref(&self) -> &str {
                    &self.0
                }
            }
        )+
    };
}

id_types! {
    /// An unvalidated game ID.
    GameId,

    /// An unvalidated stream ID.
    StreamId,

    /// An unvalidated stream tag ID.
    TagId,

    /// An unvalidated Twitch user/channel ID.
    UserId,

    /// An unvalidated Twitch video ID.
    VideoId
}

#[derive(Deserialize)]
#[allow(missing_docs)]
pub struct Game {
    pub box_art_url: Option<Url>,
    pub id: GameId,
    pub name: String
}

impl fmt::Display for Game {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.name.fmt(f)
    }
}

impl GameId {
    /// Get info about this game from the API.
    ///
    /// <https://dev.twitch.tv/docs/api/reference#get-games>
    pub async fn get(&self, client: &Client) -> Result<Game, Error> {
        Ok(
            client.get_query::<_, _, _, _, Vec<_>>("/games", &[("id", self)]).await?
            .into_iter()
            .exactly_one()?
        )
    }
}

/// Returned by `VideoId::chatlog_after_timestamp`.
#[derive(Deserialize)]
pub struct Chatlog {
    /// The messages in this part of the chatlog.
    pub comments: Vec<Message>
}

/// Part of `Chatlog`.
#[derive(Deserialize)]
pub struct Message {
    /// Not sure what this does
    pub more_replies: Json,
    /// Not sure what this does
    pub state: MessageState
}

/// Part of `Message`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageState {
    /// The only known value.
    Published
}

impl VideoId {
    /// Get the next chunk of chatlog for this video.
    ///
    /// This uses an undocumented endpoint on the old Kraken API since no equivalent functionality seems to exist in the Helix API yet.
    pub async fn chatlog_after_timestamp(&self, client: &Client, start: Duration) -> Result<Chatlog, Error> {
        client.get_raw(&format!("https://api.twitch.tv/v5/videos/{}/comments", self), vec![("content_offset_seconds", format!("{}", start.num_seconds()))]).await
    }
}

/// The type of a `Stream`, as seen in the `stream_type` field.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamType {
    /// A regular stream
    Live,
    /// Can be returned “in case of error” (whatever that means)
    #[serde(rename = "")]
    Error
}

/// A stream, as returned by <https://dev.twitch.tv/docs/api/reference#get-streams>
#[derive(Deserialize)]
#[allow(missing_docs)]
pub struct Stream {
    pub game_id: GameId,
    pub id: StreamId,
    pub language: String,
    pub started_at: DateTime<Utc>,
    pub tag_ids: Vec<TagId>, //TODO verify type
    pub thumbnail_url: Url,
    pub title: String,
    pub r#type: StreamType,
    pub user_id: UserId,
    pub user_name: String,
    pub viewer_count: u64
}

impl Stream {
    /// <https://dev.twitch.tv/docs/api/reference#get-streams>
    ///
    /// Returns a list of all streams by decreasing viewer count. The optional parameters can be used to filter down the results. `games` is limited to 10 games, and the other two are limited to 100 elements.
    pub fn list<'a>(client: &'a Client, games: Option<HashSet<GameId>>, users: Option<HashSet<UserId>>, languages: Option<HashSet<String>>) -> impl futures::Stream<Item = Result<Stream, Error>> + 'a {
        let mut query = Vec::default();
        if let Some(games) = games { query.extend(games.into_iter().map(|game_id| (format!("game_id"), game_id.0))); }
        if let Some(users) = users { query.extend(users.into_iter().map(|user_id| (format!("user_id"), user_id.0))); }
        if let Some(languages) = languages { query.extend(languages.into_iter().map(|lang_id| (format!("language"), lang_id))); }
        paginated::stream(client, format!("{}/streams", HELIX_BASE_URL), query)
    }

    /// Convenience method to get the `Game` being streamed.
    pub async fn game(&self, client: &Client) -> Result<Game, Error> {
        self.game_id.get(client).await
    }

    /// Returns a URL to this stream.
    ///
    /// Uses [this undocumented endpoint](https://discuss.dev.twitch.tv/t/url-for-live-stream-from-helix-api-data/13706).
    pub fn url(&self) -> Url {
        Url::parse(&format!("https://twitch.tv/streams/{}/channel/{}", self.id, self.user_id)).expect("could not create stream URL")
    }
}

impl fmt::Display for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.title.fmt(f)
    }
}
