//! Essentials and pinned tabs written to a file, and merged back from one.
//! A merge only ever adds: what is already in the sidebar stays as it is.
use super::*;
use zephium_core::session::bounded_name;
use zephium_core::sidebar_file::{SidebarFile, SidebarNode, SidebarSpace};

impl Shell {
    /// The focused profile's Essentials, then each space's pinned tabs, in
    /// sidebar order, each at the page it was kept at. Only ordinary web
    /// pages are written.
    pub(super) fn export_sidebar(&self) -> Option<SidebarFile> {
        let profile = self.windows.focused()?.profile;
        let spaces = self
            .spaces
            .of(profile)
            .into_iter()
            .filter_map(|space| {
                Some(SidebarSpace {
                    name: self.spaces.get(space)?.name.clone(),
                    pinned: self.sidebar_nodes(self.items.roots(Placement::Space {
                        space,
                        section: SpaceSection::Pinned,
                    })),
                })
            })
            .collect();
        let essentials = self.sidebar_nodes(self.items.roots(Placement::Favorites { profile }));
        Some(SidebarFile::new(essentials, spaces))
    }

    fn sidebar_nodes(&self, ids: &[ItemId]) -> Vec<SidebarNode> {
        ids.iter()
            .filter_map(|id| match &self.items.get(*id)?.kind {
                ItemKind::Folder { name } => Some(SidebarNode::Folder {
                    name: name.clone(),
                    children: self.sidebar_nodes(self.items.children(*id)),
                }),
                ItemKind::Tab(tab) => tab
                    .home
                    .as_ref()
                    .or(tab.url.as_ref())
                    .filter(|url| matches!(url.scheme(), "http" | "https"))
                    .map(|url| SidebarNode::Tab {
                        title: tab.title.clone(),
                        url: url.to_string(),
                    }),
            })
            .collect()
    }

    /// Adds what the file holds and the sidebar does not: Essentials and each
    /// space's pinned tabs by address, folders by name. Spaces are matched by
    /// name and made when missing. Tabs wait unloaded until opened. Returns
    /// how many tabs were added.
    pub(super) fn merge_sidebar_file(&mut self, profile: ProfileId, file: SidebarFile) -> u32 {
        let favorites = Placement::Favorites { profile };
        let mut added = self.merge_sidebar_nodes(favorites, None, file.essentials);
        for incoming in file.spaces {
            let name =
                zephium_core::spaces::space_name(&incoming.name).unwrap_or_else(|| "Space".into());
            let existing = self.spaces.of(profile).into_iter().find(|space| {
                self.spaces.get(*space).is_some_and(|space| {
                    zephium_core::spaces::space_name(&space.name).as_deref() == Some(&name)
                })
            });
            let space = match existing {
                Some(space) => space,
                None => {
                    if self.spaces.of(profile).len() >= zephium_core::spaces::MAX_PROFILE_SPACES {
                        continue;
                    }
                    let Some(space) = (0..8)
                        .map(|_| SpaceId::generate())
                        .find(|id| self.spaces.create(*id, profile, &name, Default::default()))
                    else {
                        continue;
                    };
                    space
                }
            };
            let pinned = Placement::Space {
                space,
                section: SpaceSection::Pinned,
            };
            added += self.merge_sidebar_nodes(pinned, None, incoming.pinned);
        }
        // A folder or space may be new even when no tab is.
        let _ = self.commit(Vec::new());
        added
    }

    /// Merges into one placement, whose addresses are read once up front so
    /// a repeated address, here or in the file, is added at most once.
    fn merge_sidebar_nodes(
        &mut self,
        placement: Placement,
        parent: Option<ItemId>,
        nodes: Vec<SidebarNode>,
    ) -> u32 {
        let mut seen = std::collections::HashSet::new();
        let mut pending: Vec<ItemId> = self.items.roots(placement).to_vec();
        while let Some(id) = pending.pop() {
            pending.extend_from_slice(self.items.children(id));
            if let Some(url) = self.items.tab(id).and_then(|tab| tab.url.clone()) {
                seen.insert(url);
            }
        }
        self.merge_sidebar_level(placement, parent, nodes, &mut seen)
    }

    fn merge_sidebar_level(
        &mut self,
        placement: Placement,
        parent: Option<ItemId>,
        nodes: Vec<SidebarNode>,
        seen: &mut std::collections::HashSet<url::Url>,
    ) -> u32 {
        let mut added = 0;
        for node in nodes {
            match node {
                SidebarNode::Tab { title, url } => {
                    let Some(url) = navigation::external_target(&url) else {
                        continue;
                    };
                    if seen.contains(&url) {
                        continue;
                    }
                    if (0..8).any(|_| {
                        self.items.insert_unloaded_tab_in(
                            ItemId::generate(),
                            placement,
                            parent,
                            url.clone(),
                            &title,
                        )
                    }) {
                        seen.insert(url);
                        added += 1;
                    }
                }
                SidebarNode::Folder { name, children } => {
                    let name = bounded_name(name.trim(), "Folder");
                    let siblings = match parent {
                        Some(parent) => self.items.children(parent),
                        None => self.items.roots(placement),
                    };
                    let existing = siblings.iter().copied().find(|id| {
                        matches!(
                            self.items.get(*id).map(|item| &item.kind),
                            Some(ItemKind::Folder { name: kept }) if *kept == name
                        )
                    });
                    let folder = existing.or_else(|| {
                        (0..8).map(|_| ItemId::generate()).find(|id| {
                            self.items.insert(zephium_core::item::Item {
                                id: *id,
                                parent,
                                placement,
                                kind: ItemKind::Folder { name: name.clone() },
                            })
                        })
                    });
                    if let Some(folder) = folder {
                        added += self.merge_sidebar_level(placement, Some(folder), children, seen);
                    }
                }
            }
        }
        added
    }
}
