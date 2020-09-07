//! A data structure for working with paginated endpoints

use {
    std::vec,
    futures::TryStreamExt as _,
    serde::{
        Deserialize,
        de::DeserializeOwned
    },
    crate::{
        Client,
        Error
    }
};

#[derive(Deserialize)]
#[serde(from = "Option<String>")]
enum Cursor {
    Start,
    At(String),
    End
}

impl Cursor {
    fn query(self) -> Option<Vec<(String, String)>> {
        match self {
            Cursor::Start => Some(Vec::default()),
            Cursor::At(cursor) => Some(vec![(format!("after"), cursor)]),
            Cursor::End => None // to break the loop
        }
    }
}

impl Default for Cursor {
    fn default() -> Cursor { Cursor::End }
}

impl From<Option<String>> for Cursor {
    fn from(cursor: Option<String>) -> Cursor {
        if let Some(cursor) = cursor {
            Cursor::At(cursor)
        } else {
            Cursor::End
        }
    }
}

#[derive(Default, Deserialize)]
struct PaginationInfo {
    cursor: Cursor
}

#[derive(Deserialize)]
struct PaginatedResult<T> {
    data: Vec<T>,
    #[serde(default)]
    pagination: PaginationInfo
}

pub(crate) fn stream<'a, T: DeserializeOwned>(client: &'a Client, uri: String, query: Vec<(String, String)>) -> impl futures::stream::Stream<Item = Result<T, Error>> + 'a {
    futures::stream::try_unfold(Cursor::Start, move |cursor| {
        let uri_clone = uri.clone();
        let query_clone = query.clone();
        async move {
            let query = if let Some(query) = cursor.query() {
                query
            } else {
                return Ok(None); // Cursor::End
            };
            let params = query_clone.into_iter().chain(query);
            let PaginatedResult { data, pagination }: PaginatedResult<T> = client.get_raw(&uri_clone, params).await?;
            if data.is_empty() {
                Ok::<_, Error>(None)
            } else {
                Ok(Some((futures::stream::iter(data.into_iter().map(Ok)), pagination.cursor)))
            }
        }
    }).try_flatten()
}
