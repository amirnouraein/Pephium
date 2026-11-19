//! Essentials and pinned tabs as a file a person keeps: written from one
//! profile, read back into any. A file read is untrusted, so everything in it
//! is bounded before the shell sees it, and the shell still checks each
//! address as it adds it.

use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "zephium.sidebar";
pub const VERSION: u32 = 1;
/// Far above any real export; reading stops here.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Tabs and folders together, as one session may hold.
pub const MAX_NODES: usize = crate::session::MAX_SESSION_ITEMS;
pub const MAX_SPACES: usize = crate::spaces::MAX_PROFILE_SPACES;
/// Folders nest at most this deep, counting the top level as one.
pub const MAX_DEPTH: usize = 16;
pub const MAX_URL_BYTES: usize = 8 * 1024;
pub const MAX_NAME_BYTES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarFile {
    pub format: String,
    pub version: u32,
    pub essentials: Vec<SidebarNode>,
    pub spaces: Vec<SidebarSpace>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarSpace {
    pub name: String,
    pub pinned: Vec<SidebarNode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SidebarNode {
    Tab {
        title: String,
        url: String,
    },
    Folder {
        name: String,
        children: Vec<SidebarNode>,
    },
}

/// Why a file was refused, in words chrome can put to the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarFileError {
    TooLarge,
    /// Not JSON, or not this format.
    NotSidebarFile,
    /// Written by a later Zephium.
    NewerVersion,
    TooMany,
    /// An address that is not an ordinary web page, or a name too long.
    Invalid,
}

impl SidebarFile {
    pub fn new(essentials: Vec<SidebarNode>, spaces: Vec<SidebarSpace>) -> Self {
        Self {
            format: FORMAT.into(),
            version: VERSION,
            essentials,
            spaces,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, SidebarFileError> {
        #[derive(Deserialize)]
        struct Header {
            format: String,
            version: u32,
        }
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(SidebarFileError::TooLarge);
        }
        let header: Header =
            serde_json::from_slice(bytes).map_err(|_| SidebarFileError::NotSidebarFile)?;
        if header.format != FORMAT || header.version == 0 {
            return Err(SidebarFileError::NotSidebarFile);
        }
        if header.version > VERSION {
            return Err(SidebarFileError::NewerVersion);
        }
        let file: Self =
            serde_json::from_slice(bytes).map_err(|_| SidebarFileError::NotSidebarFile)?;
        if file.spaces.len() > MAX_SPACES {
            return Err(SidebarFileError::TooMany);
        }
        let mut nodes = 0;
        check(&file.essentials, 1, &mut nodes)?;
        for space in &file.spaces {
            if space.name.len() > MAX_NAME_BYTES {
                return Err(SidebarFileError::Invalid);
            }
            check(&space.pinned, 1, &mut nodes)?;
        }
        Ok(file)
    }
}

fn check(nodes: &[SidebarNode], depth: usize, count: &mut usize) -> Result<(), SidebarFileError> {
    if depth > MAX_DEPTH {
        return Err(SidebarFileError::TooMany);
    }
    for node in nodes {
        *count += 1;
        if *count > MAX_NODES {
            return Err(SidebarFileError::TooMany);
        }
        match node {
            SidebarNode::Tab { title, url } => {
                if title.len() > MAX_NAME_BYTES
                    || url.len() > MAX_URL_BYTES
                    || crate::navigation::external_target(url).is_none()
                {
                    return Err(SidebarFileError::Invalid);
                }
            }
            SidebarNode::Folder { name, children } => {
                if name.len() > MAX_NAME_BYTES {
                    return Err(SidebarFileError::Invalid);
                }
                check(children, depth + 1, count)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(url: &str) -> SidebarNode {
        SidebarNode::Tab {
            title: "T".into(),
            url: url.into(),
        }
    }

    fn bytes(file: &SidebarFile) -> Vec<u8> {
        serde_json::to_vec(file).unwrap()
    }

    #[test]
    fn a_written_file_reads_back_the_same() {
        let file = SidebarFile::new(
            vec![tab("https://mail.example/")],
            vec![SidebarSpace {
                name: "Work".into(),
                pinned: vec![SidebarNode::Folder {
                    name: "Docs".into(),
                    children: vec![tab("http://docs.example/a")],
                }],
            }],
        );
        assert_eq!(SidebarFile::parse(&bytes(&file)), Ok(file));
    }

    #[test]
    fn bad_files_are_refused_with_a_reason() {
        use SidebarFileError::*;
        let file = |essentials: Vec<SidebarNode>| bytes(&SidebarFile::new(essentials, Vec::new()));
        let mut deep = tab("https://a.example/");
        for _ in 0..MAX_DEPTH {
            deep = SidebarNode::Folder {
                name: "F".into(),
                children: vec![deep],
            };
        }
        let cases: Vec<(Vec<u8>, SidebarFileError)> = vec![
            (vec![b' '; MAX_FILE_BYTES as usize + 1], TooLarge),
            (b"not json".to_vec(), NotSidebarFile),
            (
                br#"{"format":"other","version":1}"#.to_vec(),
                NotSidebarFile,
            ),
            (
                br#"{"format":"zephium.sidebar","version":2,"tabs":{}}"#.to_vec(),
                NewerVersion,
            ),
            (
                br#"{"format":"zephium.sidebar","version":1,"essentials":"x","spaces":[]}"#
                    .to_vec(),
                NotSidebarFile,
            ),
            (file(vec![tab("javascript:alert(1)")]), Invalid),
            (file(vec![tab("file:///etc/passwd")]), Invalid),
            (
                file(vec![tab(&format!(
                    "https://a.example/{}",
                    "a".repeat(MAX_URL_BYTES)
                ))]),
                Invalid,
            ),
            (
                file(vec![SidebarNode::Folder {
                    name: "n".repeat(MAX_NAME_BYTES + 1),
                    children: Vec::new(),
                }]),
                Invalid,
            ),
            (
                file(vec![tab("https://a.example/"); MAX_NODES + 1]),
                TooMany,
            ),
            (file(vec![deep]), TooMany),
            (
                bytes(&SidebarFile::new(
                    Vec::new(),
                    vec![
                        SidebarSpace {
                            name: "S".into(),
                            pinned: Vec::new(),
                        };
                        MAX_SPACES + 1
                    ],
                )),
                TooMany,
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(SidebarFile::parse(&input), Err(expected));
        }
    }
}
