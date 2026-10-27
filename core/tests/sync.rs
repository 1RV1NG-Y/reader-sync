//! Multi-device scenarios run in one process over real QUIC connections on this machine.
//! mDNS is off here; devices find each other through invitation addresses and the
//! addresses they learn while connected.

use std::{path::Path, time::Duration};

use bits_sync::{Node, NodeConfig, NotePatch};

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

fn body(text: &str) -> NotePatch {
    NotePatch { body: Some(text.into()), ..Default::default() }
}

fn has(node: &Node, text: &str) -> bool {
    node.notes().iter().any(|n| n.body == text)
}

fn connected(a: &Node, b: &Node) -> bool {
    let id = b.id().to_string();
    a.status().devices.iter().any(|d| d.id == id && d.connected)
}

#[tokio::test(flavor = "multi_thread")]
async fn pair_forward_merge_restart_and_remove() {
    let _ = tracing_subscriber::fmt().with_env_filter("bits_sync=debug").with_test_writer().try_init();
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

    // Notes made before pairing are kept and merged.
    a.create_note(body("from A before pairing")).await.unwrap();
    b.create_note(body("from B before pairing")).await.unwrap();

    // A invites B; B invites C. A and C never pair directly.
    let invite = a.create_invite().unwrap();
    assert_eq!(b.use_code(&invite.ticket).await.unwrap().device, "Linux");
    assert!(c.use_code(&invite.ticket).await.is_err(), "invitations are single-use");
    c.use_code(&b.create_invite().unwrap().ticket).await.unwrap();
    assert!(b.use_code(&a.create_invite().unwrap().ticket).await.is_err(), "already paired");

    wait_for("B connected to A and C", || connected(&b, &a) && connected(&b, &c)).await;
    wait_for("pre-pairing notes everywhere", || {
        [&a, &b, &c].iter().all(|n| has(n, "from A before pairing") && has(n, "from B before pairing"))
    })
    .await;
    assert_eq!(a.status().library_id, c.status().library_id);
    // Membership propagates: A now knows about C through B.
    wait_for("A trusts C", || a.status().devices.iter().any(|d| d.name == "Windows")).await;

    // Forwarding: a note from A reaches C through B.
    let note = a.create_note(body("hello from A")).await.unwrap();
    wait_for("C received A's note", || has(&c, "hello from A")).await;
    wait_for("A sees B confirmed it", || {
        a.status().devices.iter().any(|d| d.name == "Phone" && d.has_all_my_changes)
    })
    .await;
    wait_for("C knows B has C's view of everything", || {
        c.status().devices.iter().any(|d| d.name == "Phone" && d.has_all_my_changes)
    })
    .await;

    // Concurrent edits to different fields of one note combine.
    let (ra, rc) = tokio::join!(
        a.update_note(note.id.clone(), NotePatch { title: Some("Title from A".into()), ..Default::default() }),
        c.update_note(note.id.clone(), NotePatch { favorite: Some(true), ..Default::default() }),
    );
    ra.unwrap();
    rc.unwrap();
    for n in [&a, &b, &c] {
        let id = note.id.clone();
        wait_for("fields combined", || {
            n.notes().iter().any(|x| x.id == id && x.title == "Title from A" && x.favorite)
        })
        .await;
    }

    // Restart B while A keeps editing; B catches up after reopening from disk.
    b.shutdown().await;
    drop(b);
    a.create_note(body("written while B was off")).await.unwrap();
    let b = open(dirs[1].path(), "Phone").await;
    assert!(has(&b, "hello from A"), "B kept its data across the restart");
    wait_for("B reconnected and caught up", || has(&b, "written while B was off")).await;
    wait_for("C got it through B", || has(&c, "written while B was off")).await;

    // Offline edits on both sides converge after both restart.
    a.shutdown().await;
    b.shutdown().await;
    drop((a, b));
    let a = open(dirs[0].path(), "Linux").await;
    a.create_note(body("offline on A")).await.unwrap();
    a.shutdown().await;
    drop(a);
    let b = open(dirs[1].path(), "Phone").await;
    b.create_note(body("offline on B")).await.unwrap();
    let a = open(dirs[0].path(), "Linux").await;
    wait_for("offline edits converge", || {
        [&a, &b].iter().all(|n| has(n, "offline on A") && has(n, "offline on B"))
    })
    .await;

    // Deleting moves to Trash and replicates.
    let doomed = a.notes().into_iter().find(|n| n.body == "offline on A").unwrap();
    a.update_note(doomed.id.clone(), NotePatch { deleted: Some(true), ..Default::default() }).await.unwrap();
    wait_for("delete replicated to C", || c.notes().iter().any(|n| n.id == doomed.id && n.deleted)).await;

    // Removing C closes its sessions and stops further changes reaching it.
    a.remove_device(c.id().to_string()).await.unwrap();
    wait_for("B dropped C", || !connected(&b, &c)).await;
    a.create_note(body("after removing C")).await.unwrap();
    wait_for("B received it", || has(&b, "after removing C")).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!has(&c, "after removing C"));

    for n in [a, b, c] {
        n.shutdown().await;
    }
}


/// The real use: Linux and Windows are never running at the same time (dual boot), so
/// every change must travel through the phone, in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn dual_boot_handoff_through_the_phone_only() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let phone = open(dirs[1].path(), "Phone").await;

    // Linux pairs with the phone, edits, waits for "received by Phone", shuts down.
    let linux = open(dirs[0].path(), "Linux").await;
    phone.use_code(&linux.create_invite().unwrap().ticket).await.unwrap();
    let from_linux = linux.create_note(body("written on Linux")).await.unwrap();
    wait_for("phone durably has Linux's note", || {
        linux.status().devices.iter().any(|d| d.name == "Phone" && d.has_all_my_changes)
    })
    .await;
    linux.shutdown().await;
    drop(linux);

    // Windows boots and shows its code; the phone scans it (a PC can't scan). Windows then
    // receives Linux's note from the phone.
    let windows = open(dirs[2].path(), "Windows").await;
    let added = phone.use_code(&windows.create_invite().unwrap().ticket).await.unwrap();
    assert!(!added.joined && added.device == "Windows");
    wait_for("Windows got Linux's note via the phone", || has(&windows, "written on Linux")).await;
    windows
        .update_note(from_linux.id.clone(), NotePatch { title: Some("edited on Windows".into()), ..Default::default() })
        .await
        .unwrap();
    windows.create_note(body("written on Windows")).await.unwrap();
    phone.create_note(body("written on the phone between boots")).await.unwrap();
    wait_for("phone durably has Windows' changes", || {
        windows.status().devices.iter().any(|d| d.name == "Phone" && d.has_all_my_changes)
    })
    .await;
    windows.shutdown().await;
    drop(windows);

    // Linux boots again and gets everything from the phone.
    let linux = open(dirs[0].path(), "Linux").await;
    wait_for("Linux got Windows' and the phone's changes", || {
        has(&linux, "written on Windows")
            && has(&linux, "written on the phone between boots")
            && linux.notes().iter().any(|n| n.id == from_linux.id && n.title == "edited on Windows")
    })
    .await;
    // Linux learned about Windows only through the phone.
    assert!(linux.status().devices.iter().any(|d| d.name == "Windows" && !d.connected));
    linux.shutdown().await;
    phone.shutdown().await;
}
