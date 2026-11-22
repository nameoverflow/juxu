use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};

#[derive(Default, Debug)]
struct SchemaStats {
    commits: usize,
    top1: usize,
    manual: usize,
    partial: usize,
    lm_overruled: usize,
    typo: usize,
    backspaces: usize,
    abandons: usize,
}

#[derive(Clone, Debug)]
struct ParsedEvent {
    ts: u64,
    schema: String,
    event: String,
    ctx: String,
    keys: String,
    rank: usize,
    chosen: String,
    top1: String,
    typo: String,
    lm_chosen: Option<f32>,
    lm_top1: Option<f32>,
    lm_overruled: bool,
    has_touches: bool,
}

fn extract_str(line: &str, key: &str) -> String {
    let pat = format!("\"{}\":\"", key);
    let Some(start) = line.find(&pat) else {
        return String::new();
    };
    let rest = &line[start + pat.len()..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => break,
            },
            _ => out.push(c),
        }
    }
    out
}

fn extract_num(line: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{}\":", key);
    let start = line.find(&pat)? + pat.len();
    let rest = &line[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-' && c != 'e' && c != 'E' && c != '+')
        .unwrap_or(rest.len());
    rest[..end].parse::<f64>().ok()
}

fn parse_line(line: &str) -> Option<ParsedEvent> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }
    let event = extract_str(trimmed, "event");
    if event.is_empty() {
        return None;
    }
    Some(ParsedEvent {
        ts: extract_num(trimmed, "ts").unwrap_or(0.0) as u64,
        schema: extract_str(trimmed, "schema"),
        event,
        ctx: extract_str(trimmed, "ctx"),
        keys: extract_str(trimmed, "keys"),
        rank: extract_num(trimmed, "rank").unwrap_or(0.0) as usize,
        chosen: extract_str(trimmed, "chosen"),
        top1: extract_str(trimmed, "top1"),
        typo: extract_str(trimmed, "typo"),
        lm_chosen: extract_num(trimmed, "lm_chosen").map(|v| v as f32),
        lm_top1: extract_num(trimmed, "lm_top1").map(|v| v as f32),
        lm_overruled: trimmed.contains("\"lm_overruled\":true"),
        has_touches: trimmed.contains("\"touches\":["),
    })
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut inputs: Vec<String> = Vec::new();
    let mut export_tsv: Option<String> = None;
    let mut schema_filter: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--export-tsv" => {
                if i + 1 < args.len() {
                    export_tsv = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            "--schema" => {
                if i + 1 < args.len() {
                    schema_filter = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            }
            other => inputs.push(other.to_string()),
        }
        i += 1;
    }

    if inputs.is_empty() {
        if let Ok(home) = env::var("HOME") {
            let default_log = format!("{}/Library/Application Support/Juxu/eval_log.jsonl", home);
            if fs::metadata(&default_log).is_ok() {
                inputs.push(default_log);
            }
        }
    }

    if inputs.is_empty() {
        eprintln!("Usage: eval_from_log [log1.jsonl ...] [--schema pinyin] [--export-tsv out.tsv]");
        std::process::exit(1);
    }

    let mut events: Vec<ParsedEvent> = Vec::new();
    for path in &inputs {
        let Ok(file) = File::open(path) else {
            eprintln!("Warning: cannot open {}", path);
            continue;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if let Some(ev) = parse_line(&line) {
                if let Some(ref sf) = schema_filter {
                    if &ev.schema != sf {
                        continue;
                    }
                }
                events.push(ev);
            }
        }
    }

    let mut by_schema: BTreeMap<String, SchemaStats> = BTreeMap::new();
    let mut manual_misses: Vec<&ParsedEvent> = Vec::new();
    let mut retype_pairs: Vec<(&ParsedEvent, &ParsedEvent)> = Vec::new();
    let mut pending_bs: Option<&ParsedEvent> = None;

    for ev in &events {
        let st = by_schema.entry(ev.schema.clone()).or_default();
        match ev.event.as_str() {
            "commit" | "partial_commit" => {
                st.commits += 1;
                if ev.event == "partial_commit" {
                    st.partial += 1;
                }
                if ev.rank == 0 {
                    st.top1 += 1;
                } else {
                    st.manual += 1;
                    manual_misses.push(ev);
                }
                if ev.lm_overruled {
                    st.lm_overruled += 1;
                }
                if ev.typo != "none" && !ev.typo.is_empty() {
                    st.typo += 1;
                }
                if let Some(bs) = pending_bs.take() {
                    if ev.ts.saturating_sub(bs.ts) <= 10_000 && ev.chosen != bs.chosen {
                        retype_pairs.push((bs, ev));
                    }
                }
            }
            "backspace_after_commit" => {
                st.backspaces += 1;
                pending_bs = Some(ev);
            }
            "abandon" => {
                st.abandons += 1;
            }
            _ => {}
        }
    }

    println!("=========================================================================");
    println!("Juxu Real-World Input Diagnostic Report | files={} | events={}", inputs.len(), events.len());
    println!("=========================================================================");
    println!(
        "  {:<12} {:>7} {:>8} {:>7} {:>9} {:>7} {:>7} {:>7}",
        "Schema", "Commits", "Top-1%", "Manual", "LM_Overr", "Typo", "BS_Del", "Abandon"
    );
    let mut tot = SchemaStats::default();
    for (schema, s) in &by_schema {
        let top1_pct = if s.commits > 0 { 100.0 * s.top1 as f64 / s.commits as f64 } else { 0.0 };
        println!(
            "  {:<12} {:>7} {:>7.1}% {:>7} {:>9} {:>7} {:>7} {:>7}",
            schema, s.commits, top1_pct, s.manual, s.lm_overruled, s.typo, s.backspaces, s.abandons
        );
        tot.commits += s.commits;
        tot.top1 += s.top1;
        tot.manual += s.manual;
        tot.partial += s.partial;
        tot.lm_overruled += s.lm_overruled;
        tot.typo += s.typo;
        tot.backspaces += s.backspaces;
        tot.abandons += s.abandons;
    }
    let tot_top1 = if tot.commits > 0 { 100.0 * tot.top1 as f64 / tot.commits as f64 } else { 0.0 };
    println!(
        "  {:<12} {:>7} {:>7.1}% {:>7} {:>9} {:>7} {:>7} {:>7}",
        "TOTAL", tot.commits, tot_top1, tot.manual, tot.lm_overruled, tot.typo, tot.backspaces, tot.abandons
    );

    if tot.manual > 0 {
        let overr_pct = 100.0 * tot.lm_overruled as f64 / tot.manual as f64;
        println!(
            "\n  [Key Attribution] Manual non-Top-1={} | LM ALREADY preferred chosen (strategy overruled)={} ({:.1}%)",
            tot.manual, tot.lm_overruled, overr_pct
        );
    }

    if !manual_misses.is_empty() {
        println!("\n[Sample Non-Top-1 Manual Selections (up to 25)]");
        for ev in manual_misses.iter().rev().take(25) {
            let lm_info = match (ev.lm_top1, ev.lm_chosen) {
                (Some(t), Some(c)) => format!("lm_top1={:.2} lm_chosen={:.2}", t, c),
                _ => "lm=n/a".to_string(),
            };
            let tag = if ev.lm_overruled { " [STRATEGY_OVERRULED]" } else { "" };
            let touch_tag = if ev.has_touches { " [2D]" } else { "" };
            println!(
                "  [{}] ctx='{}' keys='{}' | #1='{}' -> #{}:'{}' | {} | typo={}{}{}",
                ev.schema, ev.ctx, ev.keys, ev.top1, ev.rank + 1, ev.chosen, lm_info, ev.typo, tag, touch_tag
            );
        }
    }

    if !retype_pairs.is_empty() {
        println!("\n[Backspace-After-Commit Retype Pairs (up to 20)]");
        for (bs, fix) in retype_pairs.iter().rev().take(20) {
            println!(
                "  [{}] ctx='{}' | wrong='{}' (keys='{}') -> fixed='{}' (keys='{}')",
                fix.schema, bs.ctx, bs.chosen, bs.keys, fix.chosen, fix.keys
            );
        }
    }

    if let Some(out_path) = export_tsv {
        let mut f = File::create(&out_path).expect("create export tsv");
        writeln!(f, "schema\tkind\tcontext\tkeys\tgold\twrong_top1\tlm_overruled").unwrap();
        let mut written = 0usize;
        for ev in &manual_misses {
            writeln!(
                f,
                "{}\tmanual_rank_{}\t{}\t{}\t{}\t{}\t{}",
                ev.schema,
                ev.rank + 1,
                ev.ctx,
                ev.keys,
                ev.chosen,
                ev.top1,
                ev.lm_overruled
            )
            .unwrap();
            written += 1;
        }
        for (bs, fix) in &retype_pairs {
            writeln!(
                f,
                "{}\tbackspace_retype\t{}\t{}\t{}\t{}\tfalse",
                fix.schema, bs.ctx, fix.keys, fix.chosen, bs.chosen
            )
            .unwrap();
            written += 1;
        }
        println!("\nExported {} real-world miss/retype eval cases -> {}", written, out_path);
    }
}
