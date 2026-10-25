use super::*;
use zephium_core::sidebar_file::{SidebarFile, SidebarNode, SidebarSpace};

fn tab(title: &str, url: &str) -> SidebarNode {
    SidebarNode::Tab {
        title: title.into(),
        url: url.into(),
    }
}

fn export(shell: &mut Shell) -> SidebarFile {
    let (reply, receive) = sync_channel(1);
    shell.handle(Command::ExportSidebar { reply });
    receive.recv().unwrap().unwrap()
}

fn import(shell: &mut Shell, file: SidebarFile) -> Option<u32> {
    let (send, receive) = std::sync::mpsc::channel();
    shell.handle(Command::Import {
        work: Box::new(crate::ImportWork::Sidebar(file)),
        done: crate::ImportCompletion::new(move |added| {
            let _ = send.send(added);
        }),
    });
    receive.recv().unwrap()
}

fn work_space() -> SidebarSpace {
    SidebarSpace {
        name: "Work".into(),
        pinned: vec![
            tab("Board", "https://board.example/"),
            SidebarNode::Folder {
                name: "Docs".into(),
                children: vec![
                    tab("Spec", "https://docs.example/spec"),
                    SidebarNode::Folder {
                        name: "Old".into(),
                        children: vec![tab("Wiki", "http://wiki.example/")],
                    },
                ],
            },
        ],
    }
}

fn urls(nodes: &[SidebarNode]) -> Vec<&str> {
    nodes
        .iter()
        .map(|node| match node {
            SidebarNode::Tab { url, .. } => url.as_str(),
            SidebarNode::Folder { .. } => panic!("no folder expected"),
        })
        .collect()
}

#[test]
fn the_sidebar_round_trips_through_a_file_and_merges_without_repeating() {
    let store = Arc::new(FakeStore::default());
    let (mut shell, _engine, screen) = setup_with(store.clone());
    shell.handle(Command::Bootstrap);
    let mail = active_id(&screen);
    navigate_and_commit(&mut shell, mail, "mail.example/inbox");
    shell.handle(Command::SetTabEssential {
        id: mail,
        essential: true,
        before: None,
    });
    let home = export(&mut shell).spaces[0].name.clone();

    let file = SidebarFile::new(
        vec![
            tab("Mail", "https://mail.example/inbox"),
            tab("News", "https://news.example/"),
            tab("News again", "https://news.example/"),
        ],
        vec![
            SidebarSpace {
                name: home.clone(),
                pinned: vec![tab("Music", "https://music.example/")],
            },
            work_space(),
        ],
    );
    // News, Music, and the three in Work: Mail is kept already.
    assert_eq!(import(&mut shell, file.clone()), Some(5));
    let exported = export(&mut shell);
    assert_eq!(
        urls(&exported.essentials),
        ["https://mail.example/inbox", "https://news.example/"]
    );
    assert_eq!(
        exported.spaces,
        [
            SidebarSpace {
                name: home,
                pinned: vec![tab("Music", "https://music.example/")],
            },
            work_space(),
        ]
    );
    let profile = shell.windows.focused().unwrap().profile;
    let work = shell.spaces.of(profile)[1];
    let mut pending = shell
        .items
        .roots(Placement::Space {
            space: work,
            section: SpaceSection::Pinned,
        })
        .to_vec();
    let mut tabs = 0;
    while let Some(id) = pending.pop() {
        pending.extend_from_slice(shell.items.children(id));
        if let Some(tab) = shell.items.tab(id) {
            assert!(!tab.has_view(), "an imported tab waits unloaded");
            tabs += 1;
        }
    }
    assert_eq!(tabs, 3);

    assert_eq!(import(&mut shell, exported.clone()), Some(0));
    assert_eq!(import(&mut shell, file), Some(0));
    assert_eq!(export(&mut shell), exported);

    // Into a sidebar that holds none of it, the file lands whole.
    let (mut other, _engine, _screen) = setup();
    other.handle(Command::Bootstrap);
    assert_eq!(import(&mut other, exported.clone()), Some(6));
    let landed = export(&mut other);
    assert_eq!(landed.essentials, exported.essentials);
    assert_eq!(landed.spaces[1..], exported.spaces[1..]);

    shell.persist();
    let saved = store.saved.lock().unwrap().clone().unwrap();
    let restarted = Arc::new(FakeStore::default());
    *restarted.saved.lock().unwrap() = Some(saved);
    let (mut shell, _engine, _screen) = setup_with(restarted);
    shell.handle(Command::Bootstrap);
    assert_eq!(export(&mut shell), exported);
    assert_eq!(import(&mut shell, exported), Some(0));
}

#[test]
fn only_web_pages_are_written() {
    let (mut shell, _engine, screen) = setup();
    shell.handle(Command::Bootstrap);
    let blank = active_id(&screen);
    shell.handle(Command::SetTabPinned {
        id: blank,
        pinned: true,
        before: None,
    });
    assert!(!shell
        .items
        .roots(Placement::Space {
            space: shell.windows.focused().unwrap().space,
            section: SpaceSection::Pinned,
        })
        .is_empty());
    let file = export(&mut shell);
    assert!(file.essentials.is_empty());
    assert!(file.spaces.iter().all(|space| space.pinned.is_empty()));
}
