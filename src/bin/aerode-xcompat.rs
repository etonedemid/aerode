//! `aerode-xrandr`, `aerode-xset`, `aerode-setxkbmap` (links to this binary): the small part of those
//! tools' command lines and output that Control Panel speaks, answered by the compositor. The pages
//! parse and build `xrandr`/`xset` text and that stays one place to change.
// ponytail: text protocol kept so ~10 Control Panel pages keep their parsers; move them to wmipc::Req when they are next rewritten
use aerode::wmipc::{self, Req};

fn transform_word(t: u32) -> &'static str { match t { 1 => "left", 2 => "inverted", 3 => "right", _ => "normal" } }
fn transform_of(w: &str) -> u32 { match w { "left" => 1, "inverted" => 2, "right" => 3, _ => 0 } }

fn xrandr_query() {
    let v = wmipc::call(&Req::Outputs).unwrap_or_default();
    let outs: Vec<wmipc::Output> = serde_json::from_value(v["outputs"].clone()).unwrap_or_default();
    print!("{}", format_query(&outs));
}

fn format_query(outs: &[wmipc::Output]) -> String {
    let (cw, ch) = outs.iter().filter(|o| o.enabled).fold((0, 0), |a, o| (a.0.max(o.x + o.w), a.1.max(o.y + o.h)));
    let mut s = format!("Screen 0: minimum 320 x 200, current {cw} x {ch}, maximum 16384 x 16384\n");
    let primary = outs.iter().find(|o| o.enabled).map(|o| o.name.clone());
    for o in outs {
        let geom = if o.enabled { format!(" {}x{}+{}+{} {}", o.w, o.h, o.x, o.y, if o.transform == 0 { String::new() } else { transform_word(o.transform).to_string() }) } else { String::new() };
        let prim = if primary.as_deref() == Some(&o.name) { " primary" } else { "" };
        s += &format!("{} connected{prim}{} (normal left inverted right x axis y axis) {}mm x {}mm\n", o.name, geom.trim_end(), o.phys_mm.0, o.phys_mm.1);
        let mut sizes: Vec<(i32, i32)> = vec![];
        for m in &o.modes { if !sizes.contains(&(m.0, m.1)) { sizes.push((m.0, m.1)); } }
        for (i, (w, h)) in sizes.iter().enumerate() {
            s += &format!("   {w}x{h}");
            for m in o.modes.iter().filter(|m| (m.0, m.1) == (*w, *h)) {
                let cur = o.enabled && (m.0, m.1, m.2) == (o.w, o.h, o.refresh_mhz);
                s += &format!("  {:.2}{}{}", m.2 as f64 / 1000.0, if cur { "*" } else { " " }, if i == 0 { "+" } else { " " });
            }
            s.push('\n');
        }
    }
    s
}

fn xrandr(args: &[String]) -> i32 {
    if !args.iter().any(|a| a == "--output") { xrandr_query(); return 0; }
    let mut i = 0;
    let mut cur: Option<Req> = None;
    let flush = |r: &mut Option<Req>| { if let Some(r) = r.take() { wmipc::send(r); } };
    let mut pending_rate: Option<i32> = None;
    while i < args.len() {
        let next = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--output" => {
                flush(&mut cur);
                cur = Some(Req::SetOutput { name: next, enabled: Some(true), w: None, h: None, refresh_mhz: None, x: None, y: None, scale: None, transform: None });
                i += 1;
            }
            a => {
                if let Some(Req::SetOutput { enabled, w, h, refresh_mhz, x, y, transform, .. }) = &mut cur {
                    match a {
                        "--off" => *enabled = Some(false),
                        "--mode" => { if let Some((a, b)) = next.split_once('x') { *w = a.parse().ok(); *h = b.parse().ok(); } i += 1; }
                        "--rate" => { pending_rate = next.parse::<f64>().ok().map(|r| (r * 1000.0).round() as i32); *refresh_mhz = pending_rate; i += 1; }
                        "--pos" => { if let Some((a, b)) = next.split_once('x') { *x = a.parse().ok(); *y = b.parse().ok(); } i += 1; }
                        "--rotate" => { *transform = Some(transform_of(&next)); i += 1; }
                        "--auto" | "--primary" => {}
                        // ponytail: mirroring isn't a thing the compositor does; --same-as leaves the output where it is
                        "--same-as" | "--right-of" | "--left-of" | "--above" | "--below" | "--scale" | "--brightness" | "--gamma" => i += 1,
                        _ => {}
                    }
                }
            }
        }
        i += 1;
    }
    let _ = pending_rate;
    flush(&mut cur);
    0
}

fn xset(args: &[String]) -> i32 {
    let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match a.as_slice() {
        ["q"] => {
            let v = wmipc::call(&Req::Idle).unwrap_or_default();
            let n = |k: &str| v[k].as_i64().unwrap_or(0);
            println!("Keyboard Control:\n  auto repeat:  on    key click percent:  0    LED mask:  00000000\n  auto repeat delay:  {}    repeat rate:  {}", n("repeat_delay"), n("repeat_rate"));
            println!("Screen Saver:\n  prefer blanking:  yes    allow exposures:  yes\n  timeout:  {}    cycle:  {}", n("saver"), n("saver"));
            println!("DPMS (Energy Star):\n  Standby: 0    Suspend: 0    Off: {}\n  DPMS is {}", n("dpms"), if n("dpms") > 0 { "Enabled" } else { "Disabled" });
        }
        ["r", "rate", d, r] => wmipc::send(Req::SetInput { repeat_delay: d.parse().ok(), repeat_rate: r.parse().ok(), pointer_speed: None, left_handed: None, natural_scroll: None }),
        ["s", "off"] | ["s", "0", ..] => wmipc::send(Req::SetSaverTimeout { secs: 0, cmd: String::new() }),
        ["s", n, ..] if n.parse::<u32>().is_ok() => wmipc::send(Req::SetSaverTimeout { secs: n.parse().unwrap_or(0), cmd: "aerode screensaver".into() }),
        ["-dpms"] => wmipc::send(Req::SetDpmsTimeout { secs: 0 }),
        ["+dpms", "dpms", _, _, off] | ["dpms", _, _, off] => wmipc::send(Req::SetDpmsTimeout { secs: off.parse().unwrap_or(0) }),
        ["dpms", "force", "off"] => wmipc::send(Req::DpmsOff),
        _ => {}
    }
    0
}

fn setxkbmap(args: &[String]) -> i32 {
    let st = wmipc::call(&Req::KeyboardState).unwrap_or_default();
    let cur = |k: &str| st[k].as_str().unwrap_or("").to_string();
    if args.iter().any(|a| a == "-query") {
        println!("rules:      evdev\nmodel:      pc105\nlayout:     {}\nvariant:    {}\noptions:    {}", cur("layout"), cur("variant"), cur("options"));
        return 0;
    }
    let (mut layout, mut variant, mut options) = (cur("layout"), cur("variant"), cur("options"));
    let mut i = 0;
    while i < args.len() {
        let next = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "-layout" => { layout = next; i += 1; }
            "-variant" => { variant = next; i += 1; }
            "-option" => { options = next; i += 1; }
            _ => {}
        }
        i += 1;
    }
    wmipc::send(Req::SetKeyboard { layout, variant, options });
    0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let name = std::path::Path::new(&args[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let rest = &args[1..];
    std::process::exit(match name.as_str() {
        "aerode-xrandr" => xrandr(rest),
        "aerode-xset" => xset(rest),
        "aerode-setxkbmap" => setxkbmap(rest),
        _ => { eprintln!("run as aerode-xrandr, aerode-xset or aerode-setxkbmap"); 2 }
    });
}

#[test]
fn query_text_matches_what_the_pages_parse() {
    let o = wmipc::Output { name: "DP-1".into(), enabled: true, x: 0, y: 0, w: 2560, h: 1440, refresh_mhz: 144000, modes: vec![(2560, 1440, 144000), (2560, 1440, 60000), (1920, 1080, 60000)], phys_mm: (600, 340), ..Default::default() };
    let t = format_query(&[o]);
    assert!(t.contains("current 2560 x 1440"));
    assert!(t.contains("DP-1 connected primary 2560x1440+0+0 (normal"));
    assert!(t.contains("   2560x1440  144.00*+  60.00 +"));
    assert!(t.contains("   1920x1080  60.00  "));
}
