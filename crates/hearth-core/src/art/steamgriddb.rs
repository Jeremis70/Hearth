//! SteamGridDB (API v2) as a [`MetadataProvider`], over blocking `ureq`.
//!
//! Every image request asks for static, safe images only (`types=static`, `nsfw=false`,
//! `humor=false`, `epilepsy=false`) in formats Hearth decodes, and takes the highest community
//! score. The API is a handful of GETs with a bearer key, so no client crate is used.

use std::time::Duration;

use serde::Deserialize;
use ureq::Agent;

use super::provider::{ArtKind, Asset, AssetRef, CatalogGame, MetadataProvider, ProviderError};

const API: &str = "https://www.steamgriddb.com/api/v2";
/// Square grids, our tile shape.
const SQUARE_GRIDS: &str = "512x512,1024x1024";
/// 4K heroes: Home draws the hero full-bleed behind the whole screen.
const HERO_4K: &str = "3840x1240";
/// Larger images aren't art anyone uploads on purpose.
const MAX_DOWNLOAD: u64 = 30 * 1024 * 1024;
/// How many images of one kind are offered to choose from.
const MAX_CANDIDATES: usize = 24;

/// The real provider. Holds the API key; never log it.
pub struct SteamGridDb {
    agent: Agent,
    key: String,
}

impl std::fmt::Debug for SteamGridDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SteamGridDb").finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    success: bool,
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    id: u64,
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Image {
    #[serde(default)]
    id: u64,
    url: String,
    #[serde(default)]
    thumb: String,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    mime: String,
}

impl SteamGridDb {
    /// A client using the API key `key`.
    pub fn new(key: String) -> Self {
        let agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(concat!("Hearth/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent, key }
    }

    /// GETs `path` (under the API root) and decodes the `data` list. 404 is an empty list:
    /// the API answers it for games without images of a kind.
    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, ProviderError> {
        let mut response = self
            .agent
            .get(format!("{API}{path}"))
            .header("Authorization", format!("Bearer {}", self.key))
            .call()
            .map_err(|err| ProviderError::Network(err.to_string()))?;
        let status = response.status().as_u16();
        if status == 404 {
            return Ok(Vec::new());
        }
        if status != 200 {
            return Err(ProviderError::Http {
                status,
                what: endpoint(path),
            });
        }
        let envelope: Envelope<T> = response
            .body_mut()
            .read_json()
            .map_err(|err| ProviderError::Invalid(format!("{}: {err}", endpoint(path))))?;
        if !envelope.success {
            return Err(ProviderError::Invalid(format!(
                "{}: success = false",
                endpoint(path)
            )));
        }
        Ok(envelope.data)
    }

    fn images(
        &self,
        id: u64,
        kind: ArtKind,
        dimensions: Option<&str>,
    ) -> Result<Vec<Image>, ProviderError> {
        let collection = match kind {
            ArtKind::Grid => "grids",
            ArtKind::Hero => "heroes",
            ArtKind::Logo => "logos",
            ArtKind::Icon => "icons",
        };
        let mut path = format!(
            "/{collection}/game/{id}?types=static&nsfw=false&humor=false&epilepsy=false\
             &mimes={}",
            encode(mimes(kind))
        );
        if let Some(dimensions) = dimensions {
            path.push_str(&format!("&dimensions={}", encode(dimensions)));
        }
        self.get(&path)
    }
}

impl MetadataProvider for SteamGridDb {
    fn search(&self, name: &str) -> Result<Vec<CatalogGame>, ProviderError> {
        let hits: Vec<SearchHit> = self.get(&format!("/search/autocomplete/{}", encode(name)))?;
        Ok(hits
            .into_iter()
            .map(|h| CatalogGame {
                id: h.id,
                name: h.name,
            })
            .collect())
    }

    fn best_asset(&self, id: u64, kind: ArtKind) -> Result<Option<AssetRef>, ProviderError> {
        let best = if kind == ArtKind::Grid {
            let square = self.images(id, kind, Some(SQUARE_GRIDS))?;
            if square.is_empty() {
                closest_to_square(self.images(id, kind, None)?)
            } else {
                top_scored(square)
            }
        } else if kind == ArtKind::Hero {
            let large = self.images(id, kind, Some(HERO_4K))?;
            if large.is_empty() {
                largest(self.images(id, kind, None)?)
            } else {
                top_scored(large)
            }
        } else {
            top_scored(self.images(id, kind, None)?)
        };
        Ok(best.map(|image| AssetRef {
            extension: extension(&image.mime, &image.url),
            url: image.url,
        }))
    }

    fn assets(&self, id: u64, kind: ArtKind) -> Result<Vec<Asset>, ProviderError> {
        let mut images = self.images(id, kind, None)?;
        // Best first: the highest score (the API's order breaks ties); grids square first,
        // as Home shows them square.
        let squarish = |i: &Image| kind != ArtKind::Grid || (i.width > 0 && i.width == i.height);
        images.sort_by_key(|i| (!squarish(i), std::cmp::Reverse(i.score)));
        images.truncate(MAX_CANDIDATES);
        Ok(images.into_iter().map(asset).collect())
    }

    fn download(&self, asset: &AssetRef) -> Result<Vec<u8>, ProviderError> {
        // Image URLs are on SteamGridDB's CDN: no key needed, so don't send it.
        let mut response = self
            .agent
            .get(&asset.url)
            .call()
            .map_err(|err| ProviderError::Network(err.to_string()))?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(ProviderError::Http {
                status,
                what: "image download".into(),
            });
        }
        response
            .body_mut()
            .with_config()
            .limit(MAX_DOWNLOAD)
            .read_to_vec()
            .map_err(|err| ProviderError::Network(err.to_string()))
    }
}

/// The formats to ask for: ones the `image` crate decodes here, among those the API accepts
/// for each kind (any other value is an HTTP 400). Logos are PNG or WebP only; icons are PNG
/// or `.ico`, which isn't decoded here.
fn mimes(kind: ArtKind) -> &'static str {
    match kind {
        ArtKind::Grid | ArtKind::Hero => "image/png,image/jpeg,image/webp",
        ArtKind::Logo => "image/png,image/webp",
        ArtKind::Icon => "image/png",
    }
}

/// The endpoint without ids or the search term, for error messages.
fn endpoint(path: &str) -> String {
    path.split(['?'])
        .next()
        .unwrap_or(path)
        .split('/')
        .take(3)
        .collect::<Vec<_>>()
        .join("/")
}

/// The highest score; the API's order breaks ties.
fn top_scored(images: Vec<Image>) -> Option<Image> {
    images
        .into_iter()
        .enumerate()
        .max_by_key(|(i, image)| (image.score, std::cmp::Reverse(*i)))
        .map(|(_, image)| image)
}

/// The image with the most pixels, then the highest score.
fn largest(images: Vec<Image>) -> Option<Image> {
    let area = |i: &Image| u64::from(i.width) * u64::from(i.height);
    let most = images.iter().map(area).max()?;
    top_scored(images.into_iter().filter(|i| area(i) == most).collect())
}

/// The image nearest to square (aspect ratio distance), then the highest score.
fn closest_to_square(images: Vec<Image>) -> Option<Image> {
    let distance = |image: &Image| {
        if image.width == 0 || image.height == 0 {
            return u64::MAX;
        }
        let ratio = f64::from(image.width) / f64::from(image.height);
        // In thousandths, so it can be compared exactly.
        ((ratio.ln().abs()) * 1000.0) as u64
    };
    let nearest = images.iter().map(distance).min()?;
    top_scored(
        images
            .into_iter()
            .filter(|i| distance(i) == nearest)
            .collect(),
    )
}

fn asset(image: Image) -> Asset {
    let thumb_url = if image.thumb.is_empty() {
        image.url.clone()
    } else {
        image.thumb.clone()
    };
    Asset {
        id: image.id,
        thumb: AssetRef {
            extension: extension("", &thumb_url),
            url: thumb_url,
        },
        full: AssetRef {
            extension: extension(&image.mime, &image.url),
            url: image.url,
        },
        width: image.width,
        height: image.height,
    }
}

fn extension(mime: &str, url: &str) -> String {
    match mime {
        "image/png" => "png".into(),
        "image/jpeg" => "jpg".into(),
        "image/webp" => "webp".into(),
        _ => url
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .filter(|ext| matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp"))
            .unwrap_or_else(|| "png".into()),
    }
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(url: &str, score: i64, width: u32, height: u32) -> Image {
        Image {
            id: 0,
            url: url.into(),
            thumb: String::new(),
            score,
            width,
            height,
            mime: "image/png".into(),
        }
    }

    #[test]
    fn top_score_wins_and_ties_keep_api_order() {
        let images = vec![
            image("a", 3, 1, 1),
            image("b", 9, 1, 1),
            image("c", 9, 1, 1),
        ];
        assert_eq!(top_scored(images).unwrap().url, "b");
        assert!(top_scored(Vec::new()).is_none());
    }

    #[test]
    fn grids_fall_back_to_the_closest_to_square() {
        let images = vec![
            image("wide", 50, 920, 430),
            image("portrait", 10, 600, 900),
            image("near-square", 1, 660, 620),
            image("near-square-better", 4, 620, 660),
        ];
        assert_eq!(closest_to_square(images).unwrap().url, "near-square-better");
    }

    #[test]
    fn largest_hero_when_no_4k() {
        let images = vec![
            image("small-popular", 90, 1920, 620),
            image("big", 1, 3000, 1000),
            image("big-better", 5, 3000, 1000),
        ];
        assert_eq!(largest(images).unwrap().url, "big-better");
        assert!(largest(Vec::new()).is_none());
    }

    #[test]
    fn search_terms_are_percent_encoded() {
        assert_eq!(
            encode("The Legend of Zelda: Ocarina & Co/é"),
            "The%20Legend%20of%20Zelda%3A%20Ocarina%20%26%20Co%2F%C3%A9"
        );
    }

    #[test]
    fn each_kind_asks_only_for_formats_the_api_accepts_for_it() {
        assert!(!mimes(ArtKind::Logo).contains("jpeg"));
        assert_eq!(mimes(ArtKind::Icon), "image/png");
        assert!(mimes(ArtKind::Grid).contains("jpeg"));
    }

    #[test]
    fn extensions_from_mime_or_url() {
        assert_eq!(extension("image/jpeg", "x"), "jpg");
        assert_eq!(extension("", "https://cdn/x/abc.WEBP"), "webp");
        assert_eq!(extension("", "https://cdn/x/abc.exe"), "png");
    }

    #[test]
    fn decodes_the_documented_envelope() {
        let json = r#"{"success":true,"data":[{"id":5,"score":2,"style":"alternate",
            "width":512,"height":512,"nsfw":false,"humor":false,"mime":"image/png",
            "url":"https://cdn2.steamgriddb.com/grid/x.png","thumb":"t","upvotes":2}]}"#;
        let envelope: Envelope<Image> = serde_json::from_str(json).unwrap();
        assert!(envelope.success);
        assert_eq!(envelope.data[0].width, 512);
        let offered = asset(envelope.data[0].clone());
        assert_eq!((offered.id, offered.thumb.url.as_str()), (5, "t"));
        assert_eq!(offered.full.extension, "png");
        let hits: Envelope<SearchHit> = serde_json::from_str(
            r#"{"success":true,"data":[{"id":1,"name":"Pikmin","types":["steam"],"verified":true}]}"#,
        )
        .unwrap();
        assert_eq!(hits.data[0].name, "Pikmin");
        assert_eq!(endpoint("/grids/game/42?types=static"), "/grids/game");
    }
}
