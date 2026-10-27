//! Multi-device scenarios run in one process over real QUIC connections on this machine.
//! mDNS is off here; devices find each other through invitation addresses and the
//! addresses they learn while connected.

use std::{path::Path, time::Duration};

use readersync_core::{BookPatch, Decision, ImportMeta, ImportSource, Location, NewBook, Node, NodeConfig};

async fn open(dir: &Path, name: &str) -> Node {
    Node::open(NodeConfig {
        data_dir: dir.to_owned(),
        device_name: name.into(),
        platform: "test".into(),
        port: 0,
        mdns: false,
    })
    .await
    .unwrap()
}

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !cond() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A distinct book per title (the hash is derived from the title).
fn book(title: &str) -> NewBook {
    let hash: String = (0..32).map(|i| format!("{:02x}", title.bytes().fold(i as u8, |a, b| a.wrapping_mul(31).wrapping_add(b)))).collect();
    NewBook { hash, size: 1000, format: "epub".into(), original_name: format!("{title}.epub"), title: title.into(), author: String::new() }
}

fn has(node: &Node, title: &str) -> bool {
    node.books().iter().any(|b| b.book.title == title)
}

fn at(cfi: &str) -> Location {
    Location { locator: serde_json::json!({ "cfi": cfi }), fraction: 0.5, snippet: cfi.into() }
}

fn opens_at(node: &Node, id: &str) -> String {
    let sel = node.peek_position(id).unwrap();
    sel.position.map(|p| p.record.locator["cfi"].as_str().unwrap().to_owned()).unwrap_or_default()
}

fn has_place(node: &Node, id: &str, device: &str) -> bool {
    node.handoff(id).unwrap().iter().any(|h| h.name == device && h.has_place)
}

fn connected(a: &Node, b: &Node) -> bool {
    let id = b.id().to_string();
    a.status().devices.iter().any(|d| d.id == id && d.connected)
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_forward_merge_restart_and_remove() {
    let _ = tracing_subscriber::fmt().with_env_filter("readersync_core=debug").with_test_writer().try_init();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = open(dirs[0].path(), "Linux").await;
    let b = open(dirs[1].path(), "Phone").await;
    let c = open(dirs[2].path(), "Windows").await;

    // A second process (or node) can't open the same library.
    let second = Node::open(NodeConfig {
        data_dir: dirs[0].path().to_owned(),
        device_name: "dup".into(),
        platform: "test".into(),
        port: 0,
        mdns: false,
    })
    .await;
    assert!(second.unwrap_err().to_string().contains("already using"));

    // Books added before pairing are kept and merged.
    a.add_book(book("from A before pairing")).await.unwrap();
    b.add_book(book("from B before pairing")).await.unwrap();

    // A invites B; B invites C. A and C never pair directly.
    let invite = a.create_invite().unwrap();
    assert_eq!(b.use_code(&invite.ticket).await.unwrap().device, "Linux");
    assert!(c.use_code(&invite.ticket).await.is_err(), "invitations are single-use");
    c.use_code(&b.create_invite().unwrap().ticket).await.unwrap();
    assert!(b.use_code(&a.create_invite().unwrap().ticket).await.is_err(), "already paired");

    wait_for("B connected to A and C", || connected(&b, &a) && connected(&b, &c)).await;
    wait_for("pre-pairing books everywhere", || {
        [&a, &b, &c].iter().all(|n| has(n, "from A before pairing") && has(n, "from B before pairing"))
    })
    .await;
    assert_eq!(a.status().library_id, c.status().library_id);
    // Membership propagates: A now knows about C through B.
    wait_for("A trusts C", || a.status().devices.iter().any(|d| d.name == "Windows")).await;

    // Forwarding: a book from A reaches C through B.
    let (note, _) = a.add_book(book("hello from A")).await.unwrap();
    wait_for("C received A's book", || has(&c, "hello from A")).await;
    wait_for("A sees B confirmed it", || {
        a.status().devices.iter().any(|d| d.name == "Phone" && d.has_all_my_changes)
    })
    .await;
    wait_for("C knows B has C's view of everything", || {
        c.status().devices.iter().any(|d| d.name == "Phone" && d.has_all_my_changes)
    })
    .await;

    // Concurrent edits to different fields of one book combine.
    let (ra, rc) = tokio::join!(
        a.update_book(note.id.clone(), BookPatch { title: Some("Title from A".into()), ..Default::default() }),
        c.update_book(note.id.clone(), BookPatch { author: Some("Author from C".into()), ..Default::default() }),
    );
    ra.unwrap();
    rc.unwrap();
    for n in [&a, &b, &c] {
        let id = note.id.clone();
        wait_for("fields combined", || {
            n.books().iter().any(|x| x.book.id == id && x.book.title == "Title from A" && x.book.author == "Author from C")
        })
        .await;
    }

    // Restart B while A keeps editing; B catches up after reopening from disk.
    b.shutdown().await;
    drop(b);
    a.add_book(book("written while B was off")).await.unwrap();
    let b = open(dirs[1].path(), "Phone").await;
    assert!(has(&b, "Title from A"), "B kept its data across the restart");
    wait_for("B reconnected and caught up", || has(&b, "written while B was off")).await;
    wait_for("C got it through B", || has(&c, "written while B was off")).await;

    // Offline edits on both sides converge after both restart.
    a.shutdown().await;
    b.shutdown().await;
    drop((a, b));
    let a = open(dirs[0].path(), "Linux").await;
    a.add_book(book("offline on A")).await.unwrap();
    a.shutdown().await;
    drop(a);
    let b = open(dirs[1].path(), "Phone").await;
    b.add_book(book("offline on B")).await.unwrap();
    let a = open(dirs[0].path(), "Linux").await;
    wait_for("offline edits converge", || {
        [&a, &b].iter().all(|n| has(n, "offline on A") && has(n, "offline on B"))
    })
    .await;

    // Trash replicates.
    let doomed = a.books().into_iter().find(|b| b.book.title == "offline on A").unwrap();
    a.set_trashed(vec![doomed.book.id.clone()], true).await.unwrap();
    wait_for("Trash replicated to C", || c.books().iter().any(|b| b.book.id == doomed.book.id && b.book.trashed)).await;

    // Removing C closes its sessions and stops further changes reaching it.
    a.remove_device(c.id().to_string()).await.unwrap();
    wait_for("B dropped C", || !connected(&b, &c)).await;
    a.add_book(book("after removing C")).await.unwrap();
    wait_for("B received it", || has(&b, "after removing C")).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!has(&c, "after removing C"));

    for n in [a, b, c] {
        n.shutdown().await;
    }
}


/// The real use: Linux and Windows are never running at the same time (dual boot), so the
/// book and the reading position must travel through the phone, in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn dual_boot_handoff_through_the_phone_only() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let phone = open(dirs[1].path(), "Phone").await;

    // Linux pairs with the phone, adds a book, reads, closes it (flush), waits until the
    // phone durably has that exact position, and shuts down.
    let linux = open(dirs[0].path(), "Linux").await;
    phone.use_code(&linux.create_invite().unwrap().ticket).await.unwrap();
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    let meta = ImportMeta { format: "epub".into(), original_name: "novel.epub".into(), title: "A Novel".into(), author: String::new() };
    let (b, _) = linux.import_book(ImportSource::Bytes(bytes.clone()), meta).await.unwrap();
    linux.checkpoint(b.id.clone(), at("ch1")).await.unwrap();
    linux.flush_position(b.id.clone(), at("ch3")).await.unwrap();
    // "Phone has your place": the exact position and the verified file.
    wait_for("phone durably has Linux's place and the file", || {
        linux.handoff(&b.id).unwrap().iter().any(|h| h.name == "Phone" && h.has_place && h.has_file)
    })
    .await;
    linux.shutdown().await;
    drop(linux);

    // Windows boots and shows its code; the phone scans it (a PC can't scan). Windows opens
    // the book where Linux stopped, automatically.
    let windows = open(dirs[2].path(), "Windows").await;
    phone.use_code(&windows.create_invite().unwrap().ticket).await.unwrap();
    wait_for("Windows got the book via the phone", || has(&windows, "A Novel")).await;
    wait_for("Windows got Linux's place via the phone", || opens_at(&windows, &b.id) == "ch3").await;
    let sel = windows.open_book(b.id.clone()).await.unwrap();
    assert_eq!(sel.decision, Decision::Continued);
    wait_for("Windows got the file from the phone", || windows.book_file(&b.id).unwrap().is_some()).await;
    assert_eq!(std::fs::read(windows.book_file(&b.id).unwrap().unwrap()).unwrap(), bytes);
    windows.flush_position(b.id.clone(), at("ch5")).await.unwrap();
    wait_for("phone durably has Windows' place", || has_place(&windows, &b.id, "Phone")).await;
    windows.shutdown().await;
    drop(windows);

    // The phone reads a bit between boots: a continuation of Windows' session.
    let sel = phone.open_book(b.id.clone()).await.unwrap();
    assert_eq!((sel.decision, opens_at(&phone, &b.id)), (Decision::Continued, "ch5".into()));
    phone.flush_position(b.id.clone(), at("ch6")).await.unwrap();

    // Linux boots again and continues from the phone, though it never saw Windows directly
    // and its own last position is older in every sense but the clock.
    let linux = open(dirs[0].path(), "Linux").await;
    wait_for("Linux got the phone's place", || opens_at(&linux, &b.id) == "ch6").await;
    let sel = linux.open_book(b.id.clone()).await.unwrap();
    assert_eq!(sel.decision, Decision::Continued);
    assert_eq!(sel.mine.unwrap().record.locator["cfi"], "ch3", "Back to where I was here is offered");
    // Linux learned about Windows only through the phone.
    assert!(linux.status().devices.iter().any(|d| d.name == "Windows" && !d.connected));
    linux.shutdown().await;
    phone.shutdown().await;
}
