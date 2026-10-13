//! Times `Excerpt::new` on a made-up 1,500-character body: `cargo run --release -p nm-redact --example speed`.

use std::time::Instant;

use nm_redact::{Excerpt, Known};

fn main() {
    let mut body = String::new();
    while body.chars().count() < 1_600 {
        body.push_str("Hi Ana, your order 12345 ships 2026-01-16. Pay €40 at https://pay.acme.example/i/9 \
                       or call +44 20 7946 0958; code 482910. Card 4111 1111 1111 1111, ana@acme.example. ");
    }
    let known = Known::new(["Ana Lopez", "Bo Chen"], ["you@outlook.example"]);
    let _ = Excerpt::new("warm-up", &body, &known);
    let runs = 2_000;
    let start = Instant::now();
    let mut masks = 0;
    for _ in 0..runs {
        masks += Excerpt::new("Your order 12345", &body, &known)
            .masks()
            .total();
    }
    let per = start.elapsed().as_secs_f64() * 1e3 / f64::from(runs);
    println!(
        "dense: {per:.4} ms per excerpt ({} masks each)",
        masks / runs as usize
    );

    let prose =
        "Thanks for the notes on the draft. I moved the meeting to Thursday and shared the slides \
                 with the team; let me know if anything is missing before the review. "
            .repeat(10);
    let start = Instant::now();
    for _ in 0..runs {
        let _ = Excerpt::new("Re: the draft", &prose, &known);
    }
    let per = start.elapsed().as_secs_f64() * 1e3 / f64::from(runs);
    println!("prose: {per:.4} ms per excerpt");

    let mut arabic = String::new();
    while arabic.chars().count() < 1_600 {
        arabic.push_str(
            "مرحبا محمد، طلبك رقم ١٢٣٤٥ يصل في ١٦ يناير ٢٠٢٦. المبلغ ١٬٢٥٠٫٥٠ ر.س على \
             https://pay.acme.example/i/9 أو اتصل على ٠٥٠\u{200e}-١٢٣\u{200e}-٤٥٦٧؛ رمز التحقق: ٤٨٢٩١٠. \
             شكرا لسارة والعتيبي على المساعدة، ونراكم في رمضان ١٤٤٧هـ. ",
        );
    }
    let known = Known::new(["محمد العتيبي", "سارة أحمد"], ["you@outlook.example"]);
    let start = Instant::now();
    let mut masks = 0;
    for _ in 0..runs {
        masks += Excerpt::new("طلبك رقم ١٢٣٤٥", &arabic, &known)
            .masks()
            .total();
    }
    let per = start.elapsed().as_secs_f64() * 1e3 / f64::from(runs);
    println!(
        "arabic: {per:.4} ms per excerpt ({} masks each)",
        masks / runs as usize
    );
}
