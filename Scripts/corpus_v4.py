#!/usr/bin/env python3
"""
Juxu (句序) V4 Expanded Multi-Domain Chinese Corpus Pipeline
===========================================================
Drop-in upgrade over V3 `corpus.py` (same `prepare(directory, seed)` interface,
same `clean.txt` / `tokens.bin` / `vocab.json` / `corpus.json` artifacts,
same `<COMMA>=2`, `<PERIOD>=3`, `<ENG_NUM>=4` multi-clause tokenization).

Key expansions over V3 (989M):
  1. Recent (2024-2026) internet chat, comments, and forum posts (~18% of corpus,
     trimming legacy pre-2020 LCCC from 26% -> 12%):
     - KrisTHL181/tieba-sunxiaochuan-ruozhi (2024-2026 posts & comments shards)
     - Orphanage/Baidu_Tieba_SunXiaochuan (deduplicated thread titles + replies)
     - alexxhchen/comap-weibo-bilibili-data (2024-2026 Weibo & Bilibili discussions)
     - theblackcat102/bilibili_comments_sharegpt (Bilibili comment threads)
     - FreshCrawl/rednote-xiaohongshu-notes (lifestyle/consumer notes)
     - tiagozip/bilibili (optional daily 2026 Bilibili comments when HF_TOKEN is set)
  2. Chinese technical articles & developer blogs (~18% of corpus):
     - BAAI/IndustryCorpus2_computer_programming_code (chinese/high)
     - BAAI/IndustryCorpus2_artificial_intelligence_machine_learning (chinese/high)
     - BAAI/IndustryCorpus2_computer_communication (chinese/high)
     - liwu/MNBVC (blog/ai_blog & blog/it_blog, skipping early 2013-2016 VC news stubs)

Cleaning policy:
  - NO profanity or content censorship: real everyday colloquialisms, complaints, and
    swearing are preserved intact so the IME faithfully matches what users type.
  - Only strip non-natural noise: commercial spam templates, platform UI boilerplate,
    fenced code blocks, URLs/HTML tags, and extreme single-char repetition spam.

Modes:
  - Full Cloud GPU build:
      python3 Scripts/corpus_v4.py --out-dir /path/to/out --seed-dir /path/to/seed
  - Local lightweight streaming audit (<15 MB total traffic):
      python3 Scripts/corpus_v4.py --sample-audit --out-dir ./v4_audit
"""
from __future__ import annotations

import argparse
import csv
import gzip
import hashlib
import http.client
import io
import json
import os
import re
import subprocess
import tarfile
import time
import urllib.request
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# ---------------------------------------------------------------------------
# Tokenization & Normalization (100% compatible with Juxu V3 / WBLM0001)
# ---------------------------------------------------------------------------
VOCAB = 8192
TOK_BOS = 0
TOK_UNK = 1
TOK_COMMA = 2    # ',' in clean.txt
TOK_PERIOD = 3   # '.' in clean.txt
TOK_ENG_NUM = 4  # 'E' in clean.txt
FIRST_CJK_ID = 5
MAX_PARA_CHARS = 220

START_RECENT_TS = 1704067200  # 2024-01-01 UTC
START_ZHIHU_TS = 1717200000   # 2024-06-01 UTC

RE_URL = re.compile(r"https?://\S+|www\.\S+")
RE_MD_CODE = re.compile(r"```.*?```", re.DOTALL)
RE_MD_IMG = re.compile(r"!\[[^\]]*\]\([^)]*\)")
RE_MD_LINK = re.compile(r"\[([^\]]{1,80})\]\([^)]*\)")
RE_HTML = re.compile(r"<[^>]{1,120}>")
RE_XHS_EMOJI = re.compile(r"\[[^\]]{1,10}[Rr]\]")
RE_XHS_TOPIC = re.compile(r"#[^#\n]{1,30}(?:\[话题\]#?|#)")
RE_MENTION = re.compile(r"@[\w\u4e00-\u9fff\-_.]{1,24}")
RE_TIEBA_UI = re.compile(
    r"我也说一句(?:还有\d*条回复)?|还有\d+条回复|点击查看(?:全部)?|收起回复|"
    r"来自(?:Android|iPhone|iOS|手机|网页)?客户端|贴吧用户[_A-Za-z0-9]*|"
    r"回复\s*@?[\w\u4e00-\u9fff\-_.]{1,24}\s*[:：]"
)
RE_WEIBO_UI = re.compile(
    r"[\w\u4e00-\u9fff\-]{1,24}的微博视频|收起d|展开全文|复制链接|原文[:：]?\s*$"
)
RE_WEIBO_TAIL_FULL = re.compile(r"(?:[。.！!？?\s]|^)全文\s*$")
RE_WIKI_STUB = re.compile(
    r"的一个市镇|的一个街区|的一个村|的一个社区|邮政编码|INSEE编码|"
    r"位于法国|位于意大利|位于西班牙|位于德国|位于美国[^。\n]{0,20}县|"
    r"是[^。\n]{1,20}科[^。\n]{1,20}属的|小行星|巡天系统|绕太阳运转|轨道倾角|的一座火车站|"
    r"的居民点|的自治市镇|的镇区|的市镇"
)
RE_INTER_CJK_SPACE = re.compile(r"(?<=[\u3400-\u9fff])[ \t]+(?=[\u3400-\u9fff])")
RE_INLINE_TAG = re.compile(r"<[^<>]+>|\[[^\[\]]+\]")
CLAUSE_TOK = re.compile(r"[\u3400-\u9fff]+|[A-Za-z0-9][A-Za-z0-9._+-]*|[，,、;；:：——…]+|[。.？?！!\n]+")
HAN = re.compile(r"[\u3400-\u9fff]+")
DUP_SOUND = re.compile(r"^(.)\1{5,}$")
RE_CODE_LINE = re.compile(
    r"^\s*(?:def |class |import |from |#include|public |private |const |let |var |func |fn |SELECT |INSERT |UPDATE |\{\s*$|\}\s*$)"
)

# Only commercial spam / promo boilerplate — NEVER profanity or slang
SPAM_KW = (
    "购物礼金", "优惠礼包", "京东首页搜", "淘宝搜", "加微信", "私信领",
    "扫码领", "点我头像", "点击下方链接", "复制这条信息", "打开淘宝", "领券下单",
    "关注公众号", "免费领取资料", "长按识别二维码",
)
WIKI_CUTS = ("\n参考文献\n", "\n外部链接\n", "\n参见\n", "\n参考资料\n", "\n外部连结\n")

# Target ~1.0B clean CJK chars:
#   ~30% colloquial & recent 2024-2026 social/comments (LCCC trimmed to 12%)
#   ~18% Chinese technical articles & developer blogs
#   ~24% recent 2024-2026 Zhihu Q&A
#   ~28% formal web & Wikipedia (wiki capped at 8%)
GROUP_CAPS = {
    "lccc": 120_000_000,
    "duplex": 20_000_000,
    "tieba_recent": 60_000_000,
    "weibo_bili_2026": 50_000_000,
    "bilibili_sharegpt": 25_000_000,
    "xhs_notes": 15_000_000,
    "bilibili_2026_gated": 20_000_000,
    "tech_code": 50_000_000,
    "tech_ai": 45_000_000,
    "tech_comm": 50_000_000,
    "tech_blog": 35_000_000,
    "zhihu": 240_000_000,
    "fineweb_high": 140_000_000,
    "fineweb_mid": 60_000_000,
    "wiki": 80_000_000,
}

TECH_GROUPS = {"tech_code", "tech_ai", "tech_comm", "tech_blog"}
SKIP_OPENCC = {"lccc", "duplex", "tieba_recent", "weibo_bili_2026"}


def clean_text(text: str, is_tech: bool = False) -> str:
    if not text:
        return ""
    text = RE_MD_CODE.sub(" ", text)
    text = RE_MD_IMG.sub(" ", text)
    text = RE_MD_LINK.sub(r"\1", text)
    text = RE_URL.sub(" ", text)
    text = RE_HTML.sub(" ", text)
    text = RE_XHS_EMOJI.sub(" ", text)
    text = RE_XHS_TOPIC.sub(" ", text)
    text = RE_MENTION.sub(" ", text)
    text = RE_TIEBA_UI.sub(" ", text)
    text = RE_WEIBO_UI.sub(" ", text)
    text = RE_WEIBO_TAIL_FULL.sub("", text)
    text = text.replace("`", " ")
    if is_tech:
        kept_lines = []
        for ln in text.splitlines():
            s = ln.strip()
            if not s:
                continue
            if s.startswith("|") and s.count("|") >= 3:
                continue
            if RE_CODE_LINE.match(s):
                continue
            cjk_n = sum(1 for c in s if "\u3400" <= c <= "\u9fff")
            if cjk_n < 3:
                continue
            kept_lines.append(s)
        text = "\n".join(kept_lines)
    return text


def normalize_to_multiclause(text: str) -> str:
    """Convert raw text into CJK chars plus ',' (<COMMA>), '.' (<PERIOD>), and 'E' (<ENG_NUM>)."""
    out = []
    last_punct = True
    last_eng = False
    for m in CLAUSE_TOK.finditer(text):
        tok = m.group(0)
        ch0 = tok[0]
        if "\u3400" <= ch0 <= "\u9fff":
            if DUP_SOUND.fullmatch(tok):
                continue
            out.append(tok)
            last_punct = False
            last_eng = False
        elif ch0.isascii() and ch0.isalnum():
            if not last_eng:
                out.append("E")
                last_eng = True
                last_punct = False
        elif not last_punct:
            if ch0 in "。.？?！!\n":
                out.append(".")
            else:
                out.append(",")
            last_punct = True
            last_eng = False
    while out and out[-1] in (",", ".", "E"):
        out.pop()
    while out and out[0] in (",", ".", "E"):
        out.pop(0)
    return "".join(out)


def split_paragraphs(seq: str, max_chars: int = MAX_PARA_CHARS) -> list[str]:
    if len(seq) <= max_chars:
        return [seq] if seq else []
    chunks = []
    start = 0
    n = len(seq)
    while start < n:
        end = min(n, start + max_chars)
        if end < n:
            cut = seq.rfind(".", start + max_chars // 2, end)
            if cut == -1:
                cut = seq.rfind(",", start + max_chars // 2, end)
            if cut != -1:
                end = cut
        part = seq[start:end].strip(",.")
        if len(part) >= 2:
            chunks.append(part)
        start = end + (1 if end < n and seq[end] in ",." else 0)
    return chunks


def load_eval(path: Path):
    for line in path.read_text(encoding="utf-8").splitlines():
        if "\t" in line and not line.startswith("#"):
            group, s = line.split("\t", 1)
            for part in HAN.findall(s):
                if len(part) >= 2:
                    yield group, part


def extract_eval_candidates(norm: str) -> list[str]:
    out = []
    for seg in norm.split("."):
        seg = seg.strip(",E")
        cjk = sum(1 for c in seg if "\u3400" <= c <= "\u9fff")
        if 12 <= cjk <= 44 and len(seg) <= 56:
            cjk_only = "".join(c for c in seg if "\u3400" <= c <= "\u9fff")
            if len(set(cjk_only)) >= max(7, len(cjk_only) // 3):
                out.append(seg)
    return out


# ---------------------------------------------------------------------------
# Record Parsers
# ---------------------------------------------------------------------------
def parse_tieba_sxc_jsonl(lines) -> list[str]:
    seen_titles: set[str] = set()
    out = []
    for raw in lines:
        try:
            obj = json.loads(raw)
        except Exception:
            continue
        txt = (obj.get("text") or "").strip()
        if not txt:
            continue
        if "回复：" in txt:
            head, reply = txt.split("回复：", 1)
            head = head.removeprefix("帖子：").strip()
            reply = reply.strip()
            if head and head not in seen_titles:
                seen_titles.add(head)
                out.append(head)
            if reply:
                out.append(reply)
        else:
            out.append(txt.removeprefix("帖子：").strip())
    return out


def parse_tieba_kris_jsonl(lines, min_ts: int = START_RECENT_TS):
    for raw in lines:
        try:
            obj = json.loads(raw)
        except Exception:
            continue
        try:
            ts = int(obj.get("create_time") or 0)
        except ValueError:
            ts = 0
        if min_ts and ts and ts < min_ts:
            continue
        txt = (obj.get("text") or "").strip()
        if txt:
            yield txt


def parse_mnbvc_blog_jsonl(lines, skip_early_36kr: bool = True):
    for raw in lines:
        try:
            obj = json.loads(raw)
        except Exception:
            continue
        if obj.get("是否重复文件"):
            continue
        fname = str(obj.get("文件名") or "")
        if skip_early_36kr and fname.startswith("36氪 201"):
            continue
        paras = obj.get("段落") or []
        parts = []
        for p in paras:
            if not isinstance(p, dict):
                continue
            if p.get("是否重复") or p.get("是否跨文件重复"):
                continue
            content = (p.get("内容") or "").strip()
            if content:
                parts.append(content)
        if parts:
            yield "\n".join(parts)


def parse_weibo_bili_csv(fileobj, min_year: int = 2024):
    reader = csv.DictReader(fileobj)
    for row in reader:
        created = (row.get("created_at") or "").strip()
        if created and len(created) >= 4 and created[:4].isdigit():
            if int(created[:4]) < min_year:
                continue
        title = (row.get("title") or "").strip()
        text = (row.get("text") or "").strip()
        if title and text and title not in text:
            yield f"{title}。{text}"
        elif text:
            yield text
        elif title:
            yield title


def parse_xhs_csv(fileobj):
    reader = csv.DictReader(fileobj)
    for row in reader:
        title = (row.get("title") or "").strip()
        body = (row.get("post_text") or "").strip()
        if title or body:
            yield f"{title}。{body}" if (title and body) else (title or body)


def parse_wubi86_char_keys(yaml_path: Path) -> dict[int, int]:
    char_first_key: dict[int, int] = {}
    in_body = False
    for raw in yaml_path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not in_body:
            if line == "...":
                in_body = True
            continue
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        text, code = parts[0].strip(), parts[1].strip()
        if len(text) == 1 and code and all("a" <= c <= "y" for c in code):
            cp = ord(text)
            if cp not in char_first_key:
                char_first_key[cp] = ord(code[0]) - ord("a")
    return char_first_key


# ---------------------------------------------------------------------------
# Full Cloud GPU Corpus Builder (compatible with V3 trainer)
# ---------------------------------------------------------------------------
def build_sources_list(api=None) -> list[tuple[str, str, str, str | None, bool]]:
    fineweb = "opencsg/Fineweb-Edu-Chinese-V2.1"
    sources: list[tuple[str, str, str, str | None, bool]] = [
        # 1. Recent 2024-2026 social / comments / forums
        ("tieba_recent", "Orphanage/Baidu_Tieba_SunXiaochuan", "No_train_validation_split.jsonl", None, False),
        *[
            ("tieba_recent", "KrisTHL181/tieba-sunxiaochuan-ruozhi", f"shards/posts/part-{i:05d}.jsonl.zst", None, False)
            for i in range(7, -1, -1)
        ],
        *[
            ("tieba_recent", "KrisTHL181/tieba-sunxiaochuan-ruozhi", f"shards/comments/part-{i:05d}.jsonl.zst", None, False)
            for i in range(8, -1, -1)
        ],
        ("xhs_notes", "FreshCrawl/rednote-xiaohongshu-notes", "rednote_notes.csv", None, False),
        ("bilibili_sharegpt", "theblackcat102/bilibili_comments_sharegpt", "train.jsonl", None, False),
        ("duplex", "qualialabsAI/DuplexConv", "none_Edu/jsons.tar.gz", "0bb99da7ab7a2f6f86d6b23df92c9383e711d09a", False),
        ("lccc", "silver/lccc", "lccc_large.jsonl.gz", None, False),
        # 2. Chinese technical articles & developer blogs
        ("tech_code", "BAAI/IndustryCorpus2_computer_programming_code", "chinese/high/rank_00025.parquet", None, False),
        ("tech_ai", "BAAI/IndustryCorpus2_artificial_intelligence_machine_learning", "chinese/high/rank_00011.parquet", None, False),
        ("tech_ai", "BAAI/IndustryCorpus2_artificial_intelligence_machine_learning", "chinese/high/rank_00012.parquet", None, False),
        *[
            ("tech_comm", "BAAI/IndustryCorpus2_computer_communication", f"chinese/high/rank_{i:05d}.parquet", None, False)
            for i in (17, 18, 19, 20)
        ],
        *[
            ("tech_blog", "liwu/MNBVC", f"blog/ai_blog/20230923/{i:03d}.jsonl.gz", None, False)
            for i in range(3)
        ],
        *[
            ("tech_blog", "liwu/MNBVC", f"blog/it_blog/20230923/{i:03d}.jsonl.gz", None, False)
            for i in range(1, 6)
        ],
        # 3. Modern 2024-2026 Zhihu Q&A
        *[
            ("zhihu", "KrisTHL181/zhihu-qa", f"shards/part-{i:06d}.jsonl.zst", None, False)
            for i in range(93, 34, -1)
        ],
        # 4. Formal web & Wikipedia
        *[
            ("fineweb_high", fineweb, f"4_5/{i:06d}.parquet", "a5b574efa48beb3a8f6887ef0b093becf004328b", False)
            for i in range(48)
        ],
        *[
            ("fineweb_mid", fineweb, f"3_4/{i:06d}.parquet", "a5b574efa48beb3a8f6887ef0b093becf004328b", False)
            for i in range(5)
        ],
        *[
            ("wiki", "wikimedia/wikipedia", f"20231101.zh/train-{i:05d}-of-00006.parquet", None, False)
            for i in (2, 1, 0, 3, 4, 5)
        ],
    ]
    # Dynamically add recent 2026 Weibo & Bilibili CSV files if HfApi is available
    if api is not None:
        try:
            files = api.list_repo_files("alexxhchen/comap-weibo-bilibili-data", repo_type="dataset")
            wb = sorted([f for f in files if f.startswith("weibo/processed/") and f.endswith(".csv")], reverse=True)[:120]
            bl = sorted([f for f in files if f.startswith("bilibili/processed/") and f.endswith(".csv")], reverse=True)[:80]
            insert_at = 3
            for fn in wb + bl:
                sources.insert(insert_at, ("weibo_bili_2026", "alexxhchen/comap-weibo-bilibili-data", fn, None, True))
                insert_at += 1
        except Exception as e:
            print(f"[corpus] warn: unable to list weibo/bilibili CSVs: {e}", flush=True)
    return sources


def iter_records(group: str, path: Path):
    import zstandard as zstd
    from pyarrow.parquet import ParquetFile

    if group == "duplex":
        with tarfile.open(path, "r|gz") as archive:
            for row in archive:
                if not row.isfile() or not row.name.endswith(".json"):
                    continue
                doc = json.load(archive.extractfile(row))
                parts = []
                for track in doc.get("asr", []):
                    for span in track:
                        text = (span.get("labels") or {}).get("txt")
                        if span.get("LID") == "cn" and isinstance(text, str):
                            parts.append(RE_INLINE_TAG.sub(" ", text))
                if parts:
                    yield "。".join(parts)
    elif group == "lccc":
        with gzip.open(path, "rt", encoding="utf-8", errors="replace") as f:
            for line in f:
                try:
                    value = json.loads(line)
                except Exception:
                    continue
                if isinstance(value, list):
                    turns = [RE_INTER_CJK_SPACE.sub("", t) for t in value if isinstance(t, str)]
                    if turns:
                        yield "。".join(turns)
    elif group == "tieba_recent":
        if path.name.endswith(".zst"):
            with path.open("rb") as f, zstd.ZstdDecompressor().stream_reader(f) as reader:
                with io.TextIOWrapper(reader, encoding="utf-8", errors="replace") as stream:
                    yield from parse_tieba_kris_jsonl(stream, min_ts=START_RECENT_TS)
        else:
            with path.open("r", encoding="utf-8", errors="replace") as f:
                yield from parse_tieba_sxc_jsonl(f)
    elif group == "weibo_bili_2026":
        with path.open("r", encoding="utf-8-sig", errors="replace") as f:
            yield from parse_weibo_bili_csv(f, min_year=2024)
    elif group == "xhs_notes":
        with path.open("r", encoding="utf-8-sig", errors="replace") as f:
            yield from parse_xhs_csv(f)
    elif group == "bilibili_sharegpt":
        with path.open("r", encoding="utf-8", errors="replace") as f:
            for line in f:
                try:
                    value = json.loads(line)
                except Exception:
                    continue
                if isinstance(value, dict):
                    convo = value.get("conversations") or []
                    yield "。".join(str(c.get("value") or "") for c in convo if isinstance(c, dict))
    elif group == "tech_blog":
        with gzip.open(path, "rt", encoding="utf-8", errors="replace") as f:
            yield from parse_mnbvc_blog_jsonl(f, skip_early_36kr=True)
    elif group == "zhihu":
        with path.open("rb") as f, zstd.ZstdDecompressor().stream_reader(f) as reader:
            with io.TextIOWrapper(reader, encoding="utf-8", errors="replace") as stream:
                for line in stream:
                    try:
                        answer = json.loads(line)
                    except Exception:
                        continue
                    if not isinstance(answer, dict) or (answer.get("created_time") or 0) < START_ZHIHU_TS:
                        continue
                    title = answer.get("question_title") or ""
                    yield ("" if title == "untitled" else title + "。") + (answer.get("content") or "")
    elif group == "wiki":
        for batch in ParquetFile(path).iter_batches(batch_size=512, columns=["title", "text"]):
            for row in batch.to_pylist():
                body = row.get("text") or ""
                if len(body) < 500:
                    continue
                for cut in WIKI_CUTS:
                    pos = body.find(cut)
                    if pos != -1:
                        body = body[:pos]
                yield f"{row.get('title') or ''}。{body}"
    else:
        min_len = 120 if group in ("fineweb_high", "tech_code", "tech_ai", "tech_comm") else 80
        for batch in ParquetFile(path).iter_batches(batch_size=512, columns=["text"]):
            for row in batch.to_pylist():
                text = row.get("text") or ""
                if len(text) >= min_len:
                    yield text


class CorpusWriter:
    def __init__(self, directory: Path, seed: Path):
        import ahocorasick
        from opencc import OpenCC

        self.directory = directory
        directory.mkdir(parents=True, exist_ok=True)
        self.simplifier = OpenCC("t2s")
        self.doc_seen: set[bytes] = set()
        self.para_seen: set[int] = set()
        self.stats: dict[str, Counter] = {}
        self.output = (directory / "clean.txt").open("w", buffering=1 << 21, encoding="utf-8")
        test = [self.simplifier.convert(s) for path in seed.glob("*.tsv") for _, s in load_eval(path)]
        self.test_exact = set(test)
        self.guard = ahocorasick.Automaton()
        for s in test:
            for offset in range(len(s) - 9):
                self.guard.add_word(s[offset : offset + 10], True)
        self.guard.make_automaton()

    def group_chars(self, source: str) -> int:
        return self.stats.get(source, {}).get("chars", 0)

    def add(self, source: str, data: str):
        stats = self.stats.setdefault(source, Counter())
        stats["docs"] += 1
        if not data:
            return
        if any(kw in data for kw in SPAM_KW):
            stats["spam_docs"] += 1
            return
        text = clean_text(data, is_tech=(source in TECH_GROUPS))
        if source == "wiki":
            head_s = self.simplifier.convert(text[:360])
            if RE_WIKI_STUB.search(head_s):
                stats["wiki_stub_docs"] += 1
                return
        norm = normalize_to_multiclause(text)
        if len(norm) < 2:
            return
        if source not in SKIP_OPENCC:
            norm = self.simplifier.convert(norm)
        cjk_skel = norm.replace(",", "").replace(".", "").replace("E", "")
        if len(cjk_skel) < 2:
            return
        if len(cjk_skel) >= 10 and next(self.guard.iter(cjk_skel), None) is not None:
            stats["eval_overlap_docs"] += 1
            stats["eval_overlap_chars"] += len(cjk_skel)
            return
        if cjk_skel in self.test_exact:
            stats["eval_overlap_docs"] += 1
            return
        if len(cjk_skel) >= 8:
            sig = hashlib.blake2b(cjk_skel[:512].encode(), digest_size=12).digest()
            if sig in self.doc_seen:
                stats["duplicate_doc_chars"] += len(cjk_skel)
                return
            self.doc_seen.add(sig)

        retained = []
        for para in split_paragraphs(norm, MAX_PARA_CHARS):
            p_cjk = len(para) - para.count(",") - para.count(".") - para.count("E")
            if p_cjk < 2:
                continue
            if p_cjk >= 12:
                key = hash(para)
                if key in self.para_seen:
                    stats["duplicate_para_chars"] += p_cjk
                    continue
                if len(self.para_seen) < 12_000_000:
                    self.para_seen.add(key)
            stats["chars"] += p_cjk
            stats["puncts"] += len(para) - p_cjk
            stats["paras"] += 1
            retained.append(para)
        if retained:
            self.output.write("\n".join(retained) + "\n")

    def close(self):
        self.output.close()
        return self.stats


def pack(directory: Path, seed: Path):
    import numpy as np

    chars = directory / "clean.txt"
    counts = np.zeros(0xA000, dtype=np.int64)
    outside = 0
    with chars.open(encoding="utf-8") as f:
        for text in iter(lambda: f.read(1 << 22), ""):
            points = np.frombuffer(text.encode("utf-32-le"), dtype="<u4")
            in_range = points < len(counts)
            outside += int((~in_range).sum())
            counts += np.bincount(points[in_range], minlength=len(counts))

    char_keys = parse_wubi86_char_keys(seed / "wubi86.dict.yaml")
    ranked = sorted(
        range(0x3400, 0xA000),
        key=lambda cp: (-(counts[cp] + 50 * (cp in char_keys)), cp),
    )[: VOCAB - FIRST_CJK_ID]
    vocab = sorted(ranked)
    lookup = np.ones(0xA000, dtype="<u2")
    lookup[10] = TOK_BOS
    lookup[44] = TOK_COMMA
    lookup[46] = TOK_PERIOD
    lookup[69] = TOK_ENG_NUM
    lookup[vocab] = np.arange(FIRST_CJK_ID, VOCAB, dtype=np.uint16)

    packed = directory / "tokens.bin"
    with packed.open("wb") as out_f, chars.open(encoding="utf-8") as in_f:
        for text in iter(lambda: in_f.read(1 << 22), ""):
            points = np.frombuffer(text.encode("utf-32-le"), dtype="<u4")
            selected = points < len(lookup)
            ids = np.full(points.shape, TOK_UNK, dtype="<u2")
            ids[selected] = lookup[points[selected]]
            ids.tofile(out_f)
    np.save(directory / "lut.npy", lookup[0x3400:].copy())

    all_cjk = int(counts[0x3400:].sum()) + outside
    used = int(counts[vocab].sum())
    n_comma = int(counts[44])
    n_period = int(counts[46])
    n_eng = int(counts[69])
    distribution = [1, max(1, all_cjk - used), max(1, n_comma), max(1, n_period), max(1, n_eng)] + [
        int(counts[i]) + 1 for i in vocab
    ]
    scale = float(sum(distribution))
    labels = {
        "vocab": VOCAB,
        "chars": all_cjk,
        "commas": n_comma,
        "periods": n_period,
        "eng_nums": n_eng,
        "bos": int(counts[10]),
        "tokens": packed.stat().st_size // 2,
        "oov_chars": all_cjk - used,
        "outside_lut": outside,
        "uni_logp": np.log(np.asarray(distribution) / scale).tolist(),
        "key1": [255, 255, 255, 255, 255] + [char_keys.get(cp, 255) for cp in vocab],
    }
    (directory / "vocab.json").write_text(json.dumps(labels) + "\n")
    return labels


def prepare(directory: Path, seed: Path):
    from huggingface_hub import HfApi, hf_hub_download

    if (directory / "vocab.json").is_file() and (directory / "corpus.json").is_file():
        return json.loads((directory / "vocab.json").read_text())

    started = time.monotonic()
    api = HfApi()
    raw_sources = build_sources_list(api=api)
    known: dict[str, str] = {}
    sources = []
    for group, repo, file, revision, optional in raw_sources:
        if not revision:
            if repo not in known:
                known[repo] = api.dataset_info(repo, timeout=30).sha
            revision = known[repo]
        sources.append((group, repo, file, revision, optional))

    def fetch(src):
        group, repo, file, rev, optional = src
        for attempt in range(3):
            try:
                return Path(hf_hub_download(repo, file, repo_type="dataset", revision=rev, etag_timeout=30))
            except Exception as exc:
                print(f"[download] {group}/{file} attempt={attempt+1}: {type(exc).__name__}: {exc}", flush=True)
                if attempt == 2:
                    if optional:
                        return None
                    raise
                time.sleep(2 ** attempt)

    writer = CorpusWriter(directory, seed)
    use = []
    with ThreadPoolExecutor(max_workers=2) as pool:
        active = iter(sources)

        def next_needed():
            for src in active:
                cap = GROUP_CAPS.get(src[0])
                if cap is not None and writer.group_chars(src[0]) >= cap:
                    continue
                return src
            return None

        queue = []
        for _ in range(2):
            s = next_needed()
            if s:
                queue.append((s, pool.submit(fetch, s)))

        while queue:
            source, future = queue.pop(0)
            group, repo, file, rev, optional = source
            cap = GROUP_CAPS.get(group)
            path = future.result()
            nxt = next_needed()
            if nxt:
                queue.append((nxt, pool.submit(fetch, nxt)))
            if path is None or (cap is not None and writer.group_chars(group) >= cap):
                continue
            begin = writer.group_chars(group)
            for text in iter_records(group, path):
                writer.add(group, text)
                if cap is not None and writer.group_chars(group) >= cap:
                    break
            end = writer.group_chars(group)
            use.append(
                {
                    "group": group,
                    "repo": repo,
                    "file": file,
                    "revision": rev,
                    "source_bytes": path.stat().st_size,
                    "accepted_chars": end - begin,
                }
            )
            print(f"[corpus] {group}/{file}: +{end-begin:,} chars (group={end:,}), {time.monotonic()-started:.0f}s", flush=True)
            try:
                if path.stat().st_size > 80 * 1024 * 1024:
                    path.unlink(missing_ok=True)
            except OSError:
                pass

    stats = writer.close()
    result = pack(directory, seed)
    stats_by_source = {}
    for name, row in stats.items():
        d = dict(row)
        d["avg_para_chars"] = round(d.get("chars", 0) / max(1, d.get("paras", 0)), 2)
        stats_by_source[name] = d
    overall_para = result["chars"] / max(1, result["bos"])
    manifest = {
        "sources": use,
        "groups": stats_by_source,
        "chars": result["chars"],
        "commas": result["commas"],
        "periods": result["periods"],
        "eng_nums": result["eng_nums"],
        "bos": result["bos"],
        "tokens": result["tokens"],
        "avg_para_chars": round(overall_para, 2),
        "oov_chars": result["oov_chars"],
        "outside_lut_chars": result["outside_lut"],
        "prepare_seconds": round(time.monotonic() - started),
    }
    (directory / "corpus.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    return result


# ---------------------------------------------------------------------------
# Lightweight Streaming Sample Audit (<15 MB total network traffic)
# ---------------------------------------------------------------------------
def http_range_bytes(url: str, nbytes: int) -> bytes:
    for attempt in range(3):
        req = urllib.request.Request(
            url,
            headers={"User-Agent": "Mozilla/5.0", "Range": f"bytes=0-{nbytes - 1}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return r.read()
        except http.client.IncompleteRead as e:
            if len(e.partial) >= nbytes // 2 or attempt == 2:
                return e.partial
        except Exception:
            if attempt == 2:
                raise
            time.sleep(1.5 * (attempt + 1))
    return b""


def http_get_json(url: str):
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(req, timeout=25) as r:
        return json.loads(r.read().decode("utf-8"))


def run_sample_audit(out_dir: Path):
    out_dir.mkdir(parents=True, exist_ok=True)
    eval_social: list[tuple[str, str]] = []
    eval_tech: list[tuple[str, str]] = []
    report: dict[str, dict] = {}

    def audit_stream(name: str, is_tech: bool, raw_iter, eval_bucket: list, max_docs: int = 500):
        stats = Counter()
        samples = []
        seen = set()
        for doc in raw_iter:
            if stats["accepted_docs"] >= max_docs:
                break
            stats["raw_docs"] += 1
            if not doc or any(kw in doc for kw in SPAM_KW):
                stats["spam_dropped"] += 1
                continue
            cleaned = clean_text(doc, is_tech=is_tech)
            norm = normalize_to_multiclause(cleaned)
            cjk_n = sum(1 for c in norm if "\u3400" <= c <= "\u9fff")
            if cjk_n < 4:
                stats["short_dropped"] += 1
                continue
            sig = hashlib.blake2b(norm[:256].encode("utf-8"), digest_size=8).digest()
            if sig in seen:
                stats["dedup_dropped"] += 1
                continue
            seen.add(sig)
            paras = split_paragraphs(norm, MAX_PARA_CHARS)
            stats["accepted_docs"] += 1
            for p in paras:
                pc = sum(1 for c in p if "\u3400" <= c <= "\u9fff")
                stats["chars"] += pc
                stats["eng_tokens"] += p.count("E")
                stats["commas"] += p.count(",")
                stats["periods"] += p.count(".")
                stats["paras"] += 1
            if len(samples) < 3 and cjk_n >= 12:
                samples.append({"raw": doc[:120].replace("\n", " "), "norm": paras[0][:120]})
            if stats["eval_added"] < 40:
                for cand in extract_eval_candidates(norm):
                    eval_bucket.append((name, cand))
                    stats["eval_added"] += 1
                    break
        avg_para = round(stats["chars"] / max(1, stats["paras"]), 1)
        eng_per_1k = round(1000.0 * stats["eng_tokens"] / max(1, stats["chars"]), 1)
        report[name] = {
            "raw_docs": stats["raw_docs"],
            "accepted_docs": stats["accepted_docs"],
            "dedup_dropped": stats["dedup_dropped"],
            "spam_dropped": stats["spam_dropped"],
            "chars": stats["chars"],
            "avg_para_chars": avg_para,
            "eng_per_1k_cjk": eng_per_1k,
            "samples": samples,
        }
        print(
            f"[audit] {name:22s}: docs={stats['accepted_docs']:4d}/{stats['raw_docs']:4d}, "
            f"chars={stats['chars']:7,d}, avg_para={avg_para:5.1f}, E/1k={eng_per_1k:4.1f}",
            flush=True,
        )

    b = http_range_bytes(
        "https://huggingface.co/datasets/Orphanage/Baidu_Tieba_SunXiaochuan/resolve/main/No_train_validation_split.jsonl",
        1_500_000,
    )
    audit_stream("tieba_sunxiaochuan", False, parse_tieba_sxc_jsonl(b.decode("utf-8", errors="ignore").splitlines()[:-1]), eval_social)

    b = http_range_bytes(
        "https://huggingface.co/datasets/KrisTHL181/tieba-sunxiaochuan-ruozhi/resolve/main/shards/posts/part-00007.jsonl.zst",
        1_500_000,
    )
    p = subprocess.run(["zstd", "-d", "-c"], input=b, capture_output=True)
    audit_stream(
        "tieba_kris_2025",
        False,
        parse_tieba_kris_jsonl(p.stdout.decode("utf-8", errors="ignore").splitlines()[:-1], min_ts=START_RECENT_TS),
        eval_social,
    )

    weibo_docs = []
    for fn in [
        "weibo/processed/weibo_hot_search_20121213-20261122_ac.csv",
        "weibo/processed/weibo_hot_search_20121124-20261109_ac.csv",
    ]:
        b = http_range_bytes(f"https://huggingface.co/datasets/alexxhchen/comap-weibo-bilibili-data/resolve/main/{fn}", 500_000)
        weibo_docs.extend(parse_weibo_bili_csv(io.StringIO(b.decode("utf-8-sig", errors="ignore")), min_year=2024))
    audit_stream("weibo_2026", False, weibo_docs, eval_social)

    b = http_range_bytes(
        "https://huggingface.co/datasets/FreshCrawl/rednote-xiaohongshu-notes/resolve/main/rednote_notes.csv",
        1_500_000,
    )
    audit_stream("xhs_notes", False, parse_xhs_csv(io.StringIO(b.decode("utf-8-sig", errors="ignore"))), eval_social)

    for tag, sub in [
        ("mnbvc_ai_blog", "blog/ai_blog/20230923/000.jsonl.gz"),
        ("mnbvc_it_blog", "blog/it_blog/20230923/001.jsonl.gz"),
    ]:
        b = http_range_bytes(f"https://huggingface.co/datasets/liwu/MNBVC/resolve/main/{sub}", 1_500_000)
        p = subprocess.run(["gzip", "-dc"], input=b, capture_output=True)
        audit_stream(tag, True, parse_mnbvc_blog_jsonl(p.stdout.decode("utf-8", errors="ignore").splitlines()[:-1]), eval_tech, max_docs=300)

    try:
        data = http_get_json(
            "https://datasets-server.huggingface.co/rows?dataset=BAAI/IndustryCorpus2_computer_programming_code&config=default&split=train&offset=0&length=100"
        )
        audit_stream("ic2_programming", True, [r["row"].get("text", "") for r in data.get("rows", [])], eval_tech, max_docs=100)
    except Exception as e:
        print(f"[audit] ic2_programming: SKIP ({e})", flush=True)

    (out_dir / "audit_report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    with (out_dir / "recent_social_2526.tsv").open("w", encoding="utf-8") as f:
        for dom, sent in eval_social:
            f.write(f"{dom}\t{sent}\n")
    with (out_dir / "tech_articles.tsv").open("w", encoding="utf-8") as f:
        for dom, sent in eval_tech:
            f.write(f"{dom}\t{sent}\n")
    print(f"\n[audit DONE] eval_social={len(eval_social)}, eval_tech={len(eval_tech)}", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample-audit", action="store_true")
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--seed-dir", help="Seed directory with wubi86.dict.yaml and held-out *.tsv eval sets (for full build)")
    args = ap.parse_args()
    if args.sample_audit:
        run_sample_audit(Path(args.out_dir))
    else:
        if not args.seed_dir:
            ap.error("--seed-dir is required for full corpus build")
        prepare(Path(args.out_dir), Path(args.seed_dir))


if __name__ == "__main__":
    main()
