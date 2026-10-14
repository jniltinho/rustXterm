//! M1 gate measurements at the SSH level (task 3.4, receipt in `docs/gates/m1.md`): two channels on
//! one connection, one flooding, the other used for input/echo and resize. `#[ignore = "lab"]`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use common::*;
use rxt_ssh::ShellMsg;

const FLOOD: &str = "yes \"$(head -c 200 /dev/urandom | base64 -w0)\"\n";
const FLOOD_FOR: Duration = Duration::from_secs(30);

/// `(p50, p95, max)` in milliseconds, nearest rank.
fn stats(samples: &mut [Duration]) -> (f64, f64, f64) {
    samples.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let at = |q: f64| ms(samples[((samples.len() as f64 * q).ceil() as usize).clamp(1, samples.len()) - 1]);
    (at(0.50), at(0.95), ms(samples[samples.len() - 1]))
}

fn report(name: &str, samples: &mut [Duration], limit_ms: f64) -> f64 {
    let (p50, p95, max) = stats(samples);
    println!("GATE {name}: n={} p50={p50:.2}ms p95={p95:.2}ms max={max:.2}ms limit(p95)={limit_ms}ms", samples.len());
    p95
}

/// Read `shell` until `needle` appears in its accumulated output.
async fn wait_for(shell: &mut rxt_ssh::Shell, acc: &mut String, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !acc.contains(needle) {
        match tokio::time::timeout_at(deadline, shell.read()).await {
            Ok(Some(ShellMsg::Data(d))) => acc.push_str(&String::from_utf8_lossy(&d)),
            Ok(Some(_)) => {}
            Ok(None) => panic!("channel closed waiting for {needle:?}"),
            Err(_) => panic!("timed out waiting for {needle:?}; tail: {:?}", &acc[acc.len().saturating_sub(200)..]),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "lab"]
async fn ssh_gate_flood_input_echo_resize_one_connection() {
    let s = session("alice", SSHD);
    let client = alice(SSHD).await;
    let flood = timeout(client.shell(&s, 120, 40)).await.unwrap();
    let mut sib = timeout(client.shell(&s, 120, 40)).await.unwrap();
    let (fw, sw) = (flood.writer(), sib.writer());

    // Drain the flood channel as fast as it arrives, counting bytes.
    let reader = tokio::spawn(async move {
        let mut flood = flood;
        let mut n = 0u64;
        while let Some(m) = flood.read().await {
            if let ShellMsg::Data(d) = m {
                n += d.len() as u64;
            }
        }
        n
    });
    let mut acc = String::new();
    sw.write(b"stty -echo; PS1='$ '\n").await.unwrap();
    fw.write(FLOOD.as_bytes()).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let (mut input, mut echo) = (Vec::new(), Vec::new());
    let start = Instant::now();
    let mut n = 0u32;
    while start.elapsed() < FLOOD_FOR {
        // (a) key event on the flooding channel until its bytes are handed to the transport.
        let t = Instant::now();
        fw.write(b"k").await.unwrap();
        input.push(t.elapsed());
        // (b) sibling echo: the marker only appears once the shell has evaluated it.
        n += 1;
        let t = Instant::now();
        sw.write(format!("echo E$((1000000+{n}))X\n").as_bytes()).await.unwrap();
        wait_for(&mut sib, &mut acc, &format!("E{}X", 1_000_000 + n)).await;
        echo.push(t.elapsed());
        acc.clear();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Resize the sibling while the flood keeps running.
    let (mut wc, mut stty) = (Vec::new(), Vec::new());
    for k in 0..20u16 {
        let (cols, rows) = (100 + k, 30 + k % 7);
        let t = Instant::now();
        sw.resize(cols, rows).await.unwrap();
        wc.push(t.elapsed());
        // `stty size` prints "rows cols"; retry until the remote PTY reports the new size.
        let want = format!("{rows} {cols}");
        loop {
            sw.write(b"stty size; echo SZ\n").await.unwrap();
            wait_for(&mut sib, &mut acc, "SZ\r\n").await;
            let ok = acc.contains(&want);
            acc.clear();
            if ok {
                break;
            }
            assert!(t.elapsed() < Duration::from_secs(5), "remote never reported {want}");
        }
        stty.push(t.elapsed());
    }

    fw.write(b"\x03exit\n").await.unwrap();
    sw.write(b"exit\n").await.unwrap();
    let bytes = tokio::time::timeout(Duration::from_secs(10), reader).await.ok().and_then(Result::ok).unwrap_or(0);
    println!(
        "GATE flood bytes received: {bytes} ({:.1} MiB/s)",
        bytes as f64 / 1048576.0 / start.elapsed().as_secs_f64()
    );

    let a = report("ssh input-to-transport (flooding channel)", &mut input, 100.0);
    let b = report("ssh sibling echo (same connection)", &mut echo, 100.0);
    let c = report("ssh window-change sent", &mut wc, 50.0);
    let d = report("ssh remote stty size", &mut stty, 500.0);
    assert!(a <= 100.0 && b <= 100.0, "input p95 {a} ms, echo p95 {b} ms");
    assert!(c <= 50.0, "window-change p95 {c} ms");
    // The spec bounds each resize: every sample, not only p95.
    let worst = stty.iter().max().unwrap().as_secs_f64() * 1000.0;
    assert!(worst <= 500.0 && d <= 500.0, "stty max {worst} ms");
}
