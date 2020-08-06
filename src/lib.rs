//! A Rust client for the [twitch.tv Helix API](https://dev.twitch.tv/docs/api).

#![deny(missing_docs, rust_2018_idioms, unused, unused_import_braces, unused_qualifications, warnings)]

use {
    std::{
        borrow::Borrow,
        fmt
    },
    async_std::task::sleep,
    chrono::prelude::*,
    derive_more::From,
    futures::TryFutureExt as _,
    reqwest::IntoUrl,
    serde::{
        Deserialize,
        de::DeserializeOwned
    }
};

pub mod model;
pub mod paginated;

pub(crate) const HELIX_BASE_URL: &str = "https://api.twitch.tv/helix";

/// An enum that contains all the different kinds of errors that can occur in the library.
#[derive(Debug, From)]
#[allow(missing_docs)]
pub enum Error {
    #[from(ignore)]
    ExactlyOne(bool),
    HttpStatus(reqwest::Error, reqwest::Result<String>),
    InvalidHeaderValue(reqwest::header::InvalidHeaderValue),
    Reqwest(reqwest::Error)
}

impl Error {
    fn is_spurious_network_error(&self) -> bool {
        match self {
            Error::HttpStatus(e, _) | Error::Reqwest(e) => e.status().map_or(false, |code| !code.is_client_error()),
            Error::ExactlyOne(_) | Error::InvalidHeaderValue(_) => false
        }
    }
}

impl<I: Iterator> From<itertools::ExactlyOneError<I>> for Error {
    fn from(mut e: itertools::ExactlyOneError<I>) -> Error {
        Error::ExactlyOne(e.next().is_none())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ExactlyOne(true) => write!(f, "tried to get exactly one item from an iterator but it was empty"),
            Error::ExactlyOne(false) => write!(f, "tried to get exactly one item from an iterator but it contained multiple items"),
            Error::HttpStatus(e, Ok(body)) => write!(f, "{}, body:\n\n{}", e, body),
            Error::HttpStatus(e, Err(_)) => e.fmt(f),
            Error::InvalidHeaderValue(e) => e.fmt(f),
            Error::Reqwest(e) => e.fmt(f)
        }
    }
}

/// The entry point to the API.
pub struct Client {
    client: reqwest::Client,
    /// If we're currently being rate limited, this has the time when the API can be called again.
    rate_limit_reset: Option<DateTime<Utc>>,
    token: String
}

impl Client {
    /// Constructs a new `Client` for accessing the [Helix API](https://dev.twitch.tv/docs/api).
    ///
    /// The `user_agent` parameter is used as the `User-Agent` header for all requests. It must be a `&'static str` for performance reasons.
    ///
    /// The remaining parameters of this constructor reflect that [as of April 30, 2020, all Helix endpoints will require OAuth tokens](https://discuss.dev.twitch.tv/t/requiring-oauth-for-helix-twitch-api-endpoints/23916).
    pub fn new(user_agent: &'static str, client_id: &str, oauth_token: impl fmt::Display) -> Result<Client, Error> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::USER_AGENT, reqwest::header::HeaderValue::from_static(user_agent));
        headers.insert("Client-ID", reqwest::header::HeaderValue::from_str(client_id)?);
        Ok(Client {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .build()?,
            rate_limit_reset: None,
            token: format!("{}", oauth_token)
        })
    }

    /*
    pub(crate) async fn get<U: fmt::Display, T: DeserializeOwned>(&self, url: U) -> Result<T, Error> {
        self.get_abs(&format!("{}{}", HELIX_BASE_URL, url)).await
    }

    pub(crate) async fn get_abs<U: IntoUrl, T: DeserializeOwned>(&self, url: U) -> Result<T, Error> {
        self.get_abs_query(url, &Vec::<(String, String)>::default()).await
    }
    */

    pub(crate) async fn get_query<U: fmt::Display, K: AsRef<str>, V: AsRef<str>, Q: IntoIterator, T: DeserializeOwned>(&self, url: U, query: Q) -> Result<T, Error>
    where Q::Item: Borrow<(K, V)> {
        self.get_abs_query(&format!("{}{}", HELIX_BASE_URL, url), query).await
    }

    pub(crate) async fn get_abs_query<U: IntoUrl, K: AsRef<str>, V: AsRef<str>, Q: IntoIterator, T: DeserializeOwned>(&self, url: U, query: Q) -> Result<T, Error>
    where Q::Item: Borrow<(K, V)> {
        Ok(self.get_raw::<_, _, _, _, ResponseData<_>>(url, query).await?.data)
    }

    pub(crate) async fn get_raw<U: IntoUrl, K: AsRef<str>, V: AsRef<str>, Q: IntoIterator, T: DeserializeOwned>(&self, url: U, query: Q) -> Result<T, Error>
    where Q::Item: Borrow<(K, V)>{
        let mut url = url.into_url()?;
        url.query_pairs_mut().extend_pairs(query);
        Ok(loop {
            // wait for rate limit
            if let Some(rate_limit_reset) = self.rate_limit_reset {
                if let Ok(duration) = (rate_limit_reset - Utc::now()).to_std() {
                    sleep(duration).await;
                    continue;
                }
            }
            // send request
            let response_data = self.client.get(url.clone())
                .bearer_auth(&self.token)
                .send().map_err(Error::Reqwest)
                .and_then(|resp| async {
                    match resp.error_for_status_ref() {
                        Ok(_) => Ok(resp),
                        Err(e) => Err(Error::HttpStatus(e, resp.text().await))
                    }
                })
                .await;
            match response_data {
                Ok(data) => { break data.json().await?; }
                Err(e) => if !e.is_spurious_network_error() { return Err(e); }
            }
            let response = self.client.get(url.clone())
                .bearer_auth(&self.token)
                .send().await?;
            if let Err(e) = response.error_for_status_ref() {
                return Err(Error::HttpStatus(e, response.text().await))
            }
            break response.json().await?;
        })
    }
}

#[derive(Debug, Deserialize)]
struct ResponseData<T> {
    data: T
}
