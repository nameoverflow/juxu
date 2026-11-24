#!/usr/bin/env python3
"""
Juxu (句序) V4 Mixed Chinese-English Multi-Domain Corpus Pipeline (`WBLM0002`)
=============================================================================
Builds a ~1.1B-char bilingual/mixed Chinese-English corpus + 12,288-token
mini-RWKV-World vocabulary:
  - `0`           : <BOS>
  - `1`           : <UNK> (including rare CJK outside top-8,188 / CJK Ext-B..I)
  - `2`           : <COMMA>
  - `3`           : <PERIOD>
  - `4..8191`     : Top 8,188 Simplified CJK characters (O(1) char-level LUT)
  - `8192..8286`  : All 95 visible ASCII bytes (0x20..=0x7E) -> zero <UNK> for English/digits
  - `8287..12287` : 4,001 corpus-mined English/numeric subwords (greedy longest-match Trie)

Domain budget (~1.1B chars):
  - ~28% Colloquial & recent 2024-2026 social/comments (LCCC trimmed to 120M;
         Tieba 2024-2026, Weibo/Bilibili 2026, Bilibili comments, Xiaohongshu, DuplexConv)
  - ~18% Chinese technical articles & dev blogs (rich in natural CJK + English code-switching:
         IndustryCorpus2 programming/AI/comm `chinese/high` + MNBVC AI/IT blogs)
  - ~22% Modern 2024-2026 Zhihu Q&A
  - ~22% Formal Chinese web & Wikipedia (Fineweb-Edu + zhwiki capped at 7%)
  - ~10% High-quality English tech & general prose (IndustryCorpus2 programming/AI/news/film
         `english/high`) so all 4,096 English/ASCII subwords are thoroughly trained

Cleaning policy:
  - NO profanity or content censorship.
  - Only strip commercial spam, platform UI boilerplate, fenced code blocks, URLs/HTML,
    and extreme single-char repetition spam.
  - Real English/numbers are preserved verbatim (no longer collapsed to a single `E` token).
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
import struct
import subprocess
import tarfile
import time
import urllib.request
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

# ---------------------------------------------------------------------------
# Vocabulary Layout (12,288 tokens = 128-aligned for GPU Tensor Cores)
# ---------------------------------------------------------------------------
CJK_VOCAB_LIMIT = 8192
EXT_BASE_ID = 8192
EXT_VOCAB_SIZE = 4096   # 95 visible ASCII chars + 4,001 frequent ASCII subwords
VOCAB = EXT_BASE_ID + EXT_VOCAB_SIZE  # 12,288

TOK_BOS = 0
TOK_UNK = 1
TOK_COMMA = 2
TOK_PERIOD = 3
FIRST_CJK_ID = 4
MAX_PARA_CHARS = 260

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
RE_ASCII_WORD = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,23}")
RE_MULTI_SPACE = re.compile(r"[ \t]+")
HAN = re.compile(r"[\u3400-\u9fff]+")
DUP_SOUND = re.compile(r"(.)\1{5,}")
RE_CODE_LINE = re.compile(
    r"^\s*(?:def |class |import |from |#include|public |private |const |let |var |func |fn |SELECT |INSERT |UPDATE |\{\s*$|\}\s*$)"
)

SPAM_KW = (
    "购物礼金", "优惠礼包", "京东首页搜", "淘宝搜", "加微信", "私信领",
    "扫码领", "点我头像", "点击下方链接", "复制这条信息", "打开淘宝", "领券下单",
    "关注公众号", "免费领取资料", "长按识别二维码",
)
WIKI_CUTS = ("\n参考文献\n", "\n外部链接\n", "\n参见\n", "\n参考资料\n", "\n外部连结\n")

# Target ~1.1B clean chars (CJK + English):
#   ~28% colloquial & recent 2024-2026 social/comments (~300M)
#   ~18% Chinese tech articles & blogs with dense English code-switching (~190M)
#   ~22% recent 2024-2026 Zhihu Q&A (~240M)
#   ~22% formal Chinese web & Wikipedia (~250M, wiki <=7%)
#   ~10% high-quality English tech & news/entertainment prose (~110M)
GROUP_CAPS = {
    "lccc": 55_000_000,
    "duplex": 25_000_000,
    "tieba_recent": 85_000_000,
    "weibo_bili_2026": 75_000_000,
    "bilibili_sharegpt": 35_000_000,
    "xhs_notes": 35_000_000,
    "tech_code": 60_000_000,
    "tech_ai": 50_000_000,
    "tech_comm": 55_000_000,
    "tech_blog": 40_000_000,
    "zhihu": 245_000_000,
    "fineweb_high": 140_000_000,
    "fineweb_mid": 40_000_000,
    "wiki": 50_000_000,
    # Pure English high-quality prose so all 4,096 English subwords train thoroughly
    "en_tech_code": 35_000_000,
    "en_tech_ai": 25_000_000,
    "en_news": 25_000_000,
    "en_entertainment": 25_000_000,
}

TECH_GROUPS = {"tech_code", "tech_ai", "tech_comm", "tech_blog", "en_tech_code", "en_tech_ai"}
ENGLISH_GROUPS = {"en_tech_code", "en_tech_ai", "en_news", "en_entertainment"}
SKIP_OPENCC = {"lccc", "duplex", "tieba_recent", "weibo_bili_2026"} | ENGLISH_GROUPS


# ---------------------------------------------------------------------------
# Text Cleaning & Mixed Chinese-English Normalization (Preserves Real ASCII!)
# ---------------------------------------------------------------------------
def clean_text(text: str, is_tech: bool = False, is_english: bool = False) -> str:
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
            if not is_english:
                cjk_n = sum(1 for c in s if "\u3400" <= c <= "\u9fff")
                if cjk_n < 3:
                    continue
            else:
                alpha_n = sum(1 for c in s if c.isascii() and c.isalpha())
                if alpha_n < 12 or alpha_n / max(len(s), 1) < 0.45:
                    continue
            kept_lines.append(s)
        text = "\n".join(kept_lines)
    return text


def normalize_mixed_text(text: str) -> str:
    """
    Normalize raw mixed Chinese-English text into clean lines preserving real ASCII words/digits,
    collapsing Chinese/sentence-level punctuation into ',' and '.' (matching Rust `encode_mixed_str`).
    """
    out: list[str] = []
    ascii_buf: list[str] = []
    prev_punct: str | None = None

    def flush_ascii():
        nonlocal prev_punct
        if not ascii_buf:
            return
        s = RE_MULTI_SPACE.sub(" ", "".join(ascii_buf)).strip()
        ascii_buf.clear()
        if s:
            s = DUP_SOUND.sub(lambda m: m.group(1) * 3, s)
            out.append(s)
            prev_punct = None

    n = len(text)
    for i, ch in enumerate(text):
        u = ord(ch)
        if 0x3400 <= u < 0xA000:
            flush_ascii()
            out.append(ch)
            prev_punct = None
        elif (0xF900 <= u <= 0xFAFF) or (0x20000 <= u <= 0x3FFFF):
            flush_ascii()
            out.append(ch)
            prev_punct = None
        elif ch in "，、；：—":
            flush_ascii()
            if prev_punct != ",":
                out.append(",")
                prev_punct = ","
        elif ch in "。？！\n":
            flush_ascii()
            if prev_punct != ".":
                out.append(".")
                prev_punct = "."
        elif 0x20 <= u <= 0x7E:
            next_is_alnum = (i + 1 < n) and text[i + 1].isascii() and text[i + 1].isalnum()
            if ch in ",;:" and not next_is_alnum:
                flush_ascii()
                if prev_punct != ",":
                    out.append(",")
                    prev_punct = ","
            elif ch in ".?!" and not next_is_alnum:
                flush_ascii()
                if prev_punct != ".":
                    out.append(".")
                    prev_punct = "."
            else:
                ascii_buf.append(ch)
        elif ch.isspace():
            if ascii_buf:
                ascii_buf.append(" ")
    flush_ascii()
    res = "".join(out)
    res = DUP_SOUND.sub(lambda m: m.group(1) * 3, res)
    return res.strip(",. ")


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
            if cut == -1:
                cut = seq.rfind(" ", start + max_chars // 2, end)
            if cut != -1:
                end = cut
        part = seq[start:end].strip(",. ")
        if len(part) >= 4:
            chunks.append(part)
        start = end + (1 if end < n and seq[end] in ",. " else 0)
    return chunks


# ---------------------------------------------------------------------------
# Mini-RWKV-World ASCII Subword Vocab Builder & Greedy Longest-Match Trie
# ---------------------------------------------------------------------------
class AsciiTrie:
    """Bit-exact Python twin of Rust `Core/src/neural.rs::AsciiTrie`."""

    def __init__(self, ext_base_id: int, entries: list[str]):
        self.ext_base_id = ext_base_id
        self.entries = entries
        self.root: dict[int, dict] = {}
        self.single_ascii = [TOK_UNK] * 95
        for idx, s in enumerate(entries):
            b = s.encode("ascii")
            tok = ext_base_id + idx
            if len(b) == 1 and 0x20 <= b[0] <= 0x7E:
                self.single_ascii[b[0] - 0x20] = tok
            node = self.root
            for ch_b in b:
                nxt = node.get(ch_b)
                if nxt is None:
                    nxt = {}
                    node[ch_b] = nxt
                node = nxt
            node[-1] = tok

    def encode_ascii(self, data: bytes) -> list[int]:
        out: list[int] = []
        i = 0
        n = len(data)
        root = self.root
        single = self.single_ascii
        while i < n:
            node = root
            best_tok = 0
            best_len = 0
            j = i
            while j < n:
                nxt = node.get(data[j])
                if nxt is None:
                    break
                node = nxt
                j += 1
                t = node.get(-1)
                if t is not None:
                    best_tok = t
                    best_len = j - i
            if best_len > 0:
                out.append(best_tok)
                i += best_len
            else:
                b = data[i]
                out.append(single[b - 0x20] if 0x20 <= b <= 0x7E else TOK_UNK)
                i += 1
        return out


def build_ascii_subword_vocab(ascii_word_counts: Counter[str], target_size: int = EXT_VOCAB_SIZE) -> list[str]:
    """
    Build a 4,096-entry ASCII subword vocabulary:
      - First 95 entries: all visible ASCII single bytes (0x20..=0x7E)
      - Remaining 4,001 entries: BPE-mined frequent English/numeric words & subwords
    """
    base_chars = [chr(c) for c in range(0x20, 0x7F)]
    seen = set(base_chars)
    subwords: list[str] = list(base_chars)
    # Pin high-frequency mixed CJK+English dev/device words in both lowercase and canonical casing
    # so stripped bare ASCII spans right after CJK characters ALWAYS hit single whole-word tokens.
    pinned_bare = [
        "review", "commit", "commits", "push", "pull", "merge", "rebase", "branch", "diff", "patch",
        "iphone", "iPhone", "macos", "macOS", "ios", "iOS", "ipad", "iPad", "xcode", "Xcode",
        "claude", "Claude", "gemini", "Gemini", "qwen", "Qwen", "rwkv", "RWKV", "gpt", "GPT",
        "github", "GitHub", "docker", "Docker", "python", "Python", "rust", "Rust", "swift", "Swift",
        "linux", "Linux", "cuda", "CUDA", "vllm", "sglang", "colab", "mmap", "token", "tokens",
        "model", "models", "config", "debug", "release", "build", "test", "tests", "fix", "feat",
        "bug", "api", "API", "cli", "CLI", "app", "App", "web", "gpu", "GPU", "cpu", "CPU",
    ]
    for w in pinned_bare:
        if len(subwords) < target_size and w not in seen and all(0x20 <= ord(c) <= 0x7E for c in w):
            seen.add(w)
            subwords.append(w)
    need = target_size - len(subwords)
    if need <= 0 or not ascii_word_counts:
        return subwords[:target_size]

    # Seed with top-25k frequent ASCII words/tokens for fast BPE merge mining
    vocab_seqs: list[tuple[list[str], int]] = []
    for w, cnt in ascii_word_counts.most_common(25000):
        if cnt < 2 or not w:
            continue
        if all(0x20 <= ord(c) <= 0x7E for c in w):
            vocab_seqs.append((list(w), cnt))

    # Fast iterative BPE pair merges (batch top non-overlapping merges per round for speed)
    while len(subwords) < target_size and vocab_seqs:
        pair_counts: Counter[tuple[str, str]] = Counter()
        for seq, cnt in vocab_seqs:
            for k in range(len(seq) - 1):
                pair_counts[(seq[k], seq[k + 1])] += cnt
        if not pair_counts:
            break
        # Pick up to 64 top non-overlapping pairs per round so 4,000 merges take ~2-4 seconds
        batch_pairs: dict[tuple[str, str], str] = {}
        used_syms: set[str] = set()
        for (a, b), c in pair_counts.most_common(256):
            if c < 2:
                break
            merged = a + b
            if len(merged) > 16 or merged in seen:
                continue
            if a in used_syms or b in used_syms:
                continue
            batch_pairs[(a, b)] = merged
            used_syms.add(a)
            used_syms.add(b)
            seen.add(merged)
            subwords.append(merged)
            if len(subwords) >= target_size or len(batch_pairs) >= 64:
                break
        if not batch_pairs:
            break
        new_seqs = []
        for seq, cnt in vocab_seqs:
            out_s: list[str] = []
            k = 0
            ln = len(seq)
            while k < ln:
                if k + 1 < ln:
                    m = batch_pairs.get((seq[k], seq[k + 1]))
                    if m is not None:
                        out_s.append(m)
                        k += 2
                        continue
                out_s.append(seq[k])
                k += 1
            if len(out_s) > 1:
                new_seqs.append((out_s, cnt))
        vocab_seqs = new_seqs

    return subwords[:target_size]


def encode_mixed_line(text: str, cjk_lut: dict[int, int], trie: AsciiTrie) -> list[int]:
    """Bit-exact Python twin of Rust `NeuralView::encode_mixed_str`."""
    out: list[int] = []
    ascii_buf: list[int] = []
    prev_punct: int | None = None

    def flush_ascii():
        nonlocal prev_punct
        if not ascii_buf:
            return
        norm: list[int] = []
        last_sp = True
        for b in ascii_buf:
            if b == 0x20:
                if not last_sp:
                    norm.append(0x20)
                    last_sp = True
            else:
                norm.append(b)
                last_sp = False
        while norm and norm[-1] == 0x20:
            norm.pop()
        ascii_buf.clear()
        if norm:
            out.extend(trie.encode_ascii(bytes(norm)))
            prev_punct = None

    n = len(text)
    for i, ch in enumerate(text):
        u = ord(ch)
        if 0x3400 <= u < 0xA000:
            flush_ascii()
            out.append(cjk_lut.get(u, TOK_UNK))
            prev_punct = None
        elif (0xF900 <= u <= 0xFAFF) or (0x20000 <= u <= 0x3FFFF):
            flush_ascii()
            out.append(TOK_UNK)
            prev_punct = None
        elif ch in "，、；：—":
            flush_ascii()
            if prev_punct != TOK_COMMA:
                out.append(TOK_COMMA)
                prev_punct = TOK_COMMA
        elif ch in "。？！\n":
            flush_ascii()
            if prev_punct != TOK_PERIOD:
                out.append(TOK_PERIOD)
                prev_punct = TOK_PERIOD
        elif 0x20 <= u <= 0x7E:
            next_is_alnum = (i + 1 < n) and text[i + 1].isascii() and text[i + 1].isalnum()
            if ch in ",;:" and not next_is_alnum:
                flush_ascii()
                if prev_punct != TOK_COMMA:
                    out.append(TOK_COMMA)
                    prev_punct = TOK_COMMA
            elif ch in ".?!" and not next_is_alnum:
                flush_ascii()
                if prev_punct != TOK_PERIOD:
                    out.append(TOK_PERIOD)
                    prev_punct = TOK_PERIOD
            else:
                ascii_buf.append(u)
        elif ch.isspace():
            if ascii_buf:
                ascii_buf.append(0x20)
    flush_ascii()
    return out


def pack_wblm0002_ext_vocab_bytes(ext_base_id: int, ext_vocab: list[str]) -> bytes:
    """Serialize trailing `EXT_VOCAB` section for `WBLM0002` binary export."""
    buf = bytearray(struct.pack("<II", ext_base_id, len(ext_vocab)))
    for s in ext_vocab:
        b = s.encode("ascii")
        assert 1 <= len(b) <= 255
        buf.append(len(b))
        buf.extend(b)
    return bytes(buf)


# ---------------------------------------------------------------------------
# Record Parsers & Cloud GPU Sources List
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
        # 2. Chinese technical articles & dev blogs (dense CJK + English code-switching)
        ("tech_code", "BAAI/IndustryCorpus2_computer_programming_code", "chinese/high/rank_00025.parquet", None, True),
        ("tech_code", "BAAI/IndustryCorpus2_computer_programming_code", "chinese/high/rank_00024.parquet", None, True),
        ("tech_ai", "BAAI/IndustryCorpus2_artificial_intelligence_machine_learning", "chinese/high/rank_00011.parquet", None, True),
        ("tech_ai", "BAAI/IndustryCorpus2_artificial_intelligence_machine_learning", "chinese/high/rank_00012.parquet", None, True),
        ("tech_ai", "BAAI/IndustryCorpus2_artificial_intelligence_machine_learning", "chinese/high/rank_00010.parquet", None, True),
        ("tech_ai", "BAAI/IndustryCorpus2_artificial_intelligence_machine_learning", "chinese/high/rank_00013.parquet", None, True),
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
        # 3. High-quality English tech & news/entertainment prose (~10% of total corpus)
        ("en_tech_code", "BAAI/IndustryCorpus2_computer_programming_code", "english/high/rank_00864.parquet", None, False),
        ("en_tech_ai", "BAAI/IndustryCorpus2_artificial_intelligence_machine_learning", "english/high/rank_00785.parquet", None, False),
        ("en_news", "BAAI/IndustryCorpus2_news_media", "english/high/rank_01332.parquet", None, False),
        ("en_entertainment", "BAAI/IndustryCorpus2_film_entertainment", "english/high/rank_00961.parquet", None, False),
        # 4. Modern 2024-2026 Zhihu Q&A
        *[
            ("zhihu", "KrisTHL181/zhihu-qa", f"shards/part-{i:06d}.jsonl.zst", None, False)
            for i in range(93, 34, -1)
        ],
        # 5. Formal Chinese web & Wikipedia
        *[
            ("fineweb_high", fineweb, f"4_5/{i:06d}.parquet", "a5b574efa48beb3a8f6887ef0b093becf004328b", False)
            for i in range(44)
        ],
        *[
            ("fineweb_mid", fineweb, f"3_4/{i:06d}.parquet", "a5b574efa48beb3a8f6887ef0b093becf004328b", False)
            for i in range(4)
        ],
        *[
            ("wiki", "wikimedia/wikipedia", f"20231101.zh/train-{i:05d}-of-00006.parquet", None, False)
            for i in (2, 1, 0, 3, 4, 5)
        ],
    ]
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


def load_eval(path: Path):
    for line in path.read_text(encoding="utf-8").splitlines():
        if "\t" in line and not line.startswith("#"):
            group, s = line.split("\t", 1)
            for part in HAN.findall(s):
                if len(part) >= 2:
                    yield group, part


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
        min_len = 120 if group in ({"fineweb_high"} | TECH_GROUPS | ENGLISH_GROUPS) else 80
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
        is_en = source in ENGLISH_GROUPS
        text = clean_text(data, is_tech=(source in TECH_GROUPS), is_english=is_en)
        if source == "wiki":
            head_s = self.simplifier.convert(text[:360])
            if RE_WIKI_STUB.search(head_s):
                stats["wiki_stub_docs"] += 1
                return
        norm = normalize_mixed_text(text)
        if len(norm) < 4:
            return
        if source not in SKIP_OPENCC:
            norm = self.simplifier.convert(norm)
        cjk_skel = "".join(c for c in norm if "\u3400" <= c <= "\u9fff")
        if not is_en and len(cjk_skel) < 2:
            return
        if len(cjk_skel) >= 10 and next(self.guard.iter(cjk_skel), None) is not None:
            stats["eval_overlap_docs"] += 1
            return
        if cjk_skel and cjk_skel in self.test_exact:
            stats["eval_overlap_docs"] += 1
            return
        sig_src = cjk_skel[:512] if len(cjk_skel) >= 8 else norm[:512]
        if len(sig_src) >= 8:
            sig = hashlib.blake2b(sig_src.encode(), digest_size=12).digest()
            if sig in self.doc_seen:
                stats["duplicate_doc_chars"] += len(norm)
                return
            self.doc_seen.add(sig)

        retained = []
        for para in split_paragraphs(norm, MAX_PARA_CHARS):
            if len(para) < 4:
                continue
            if len(para) >= 16:
                key = hash(para)
                if key in self.para_seen:
                    stats["duplicate_para_chars"] += len(para)
                    continue
                if len(self.para_seen) < 12_000_000:
                    self.para_seen.add(key)
            stats["chars"] += len(para)
            stats["paras"] += 1
            retained.append(para)
        if retained:
            self.output.write("\n".join(retained) + "\n")

    def close(self):
        self.output.close()
        return self.stats


def pack(directory: Path, seed: Path):
    import numpy as np

    clean_path = directory / "clean.txt"
    cjk_counts = np.zeros(0xA000, dtype=np.int64)
    ascii_words: Counter[str] = Counter()
    total_lines = 0
    with clean_path.open(encoding="utf-8") as f:
        for line in f:
            s = line.strip()
            if not s:
                continue
            total_lines += 1
            for ch in s:
                u = ord(ch)
                if 0x3400 <= u < 0xA000:
                    cjk_counts[u] += 1
            for m in RE_ASCII_WORD.finditer(s):
                ascii_words[m.group(0)] += 1
            if len(ascii_words) > 400_000:
                ascii_words = Counter(dict(ascii_words.most_common(120_000)))

    ranked_cjk = sorted(
        range(0x3400, 0xA000),
        key=lambda cp: (-int(cjk_counts[cp]), cp),
    )[: CJK_VOCAB_LIMIT - FIRST_CJK_ID]
    vocab_cjk = sorted(ranked_cjk)
    cjk_lut_arr = np.ones(0xA000 - 0x3400, dtype="<u2")
    cjk_lut_map: dict[int, int] = {}
    for idx, cp in enumerate(vocab_cjk):
        tid = FIRST_CJK_ID + idx
        cjk_lut_arr[cp - 0x3400] = tid
        cjk_lut_map[cp] = tid

    ext_vocab = build_ascii_subword_vocab(ascii_words, target_size=EXT_VOCAB_SIZE)
    trie = AsciiTrie(EXT_BASE_ID, ext_vocab)
    vocab_total = EXT_BASE_ID + len(ext_vocab)

    tok_counts = np.ones(vocab_total, dtype=np.int64)
    packed = directory / "tokens.bin"
    total_tokens = 0
    with packed.open("wb") as out_f, clean_path.open(encoding="utf-8") as in_f:
        buf = bytearray()
        for line in in_f:
            s = line.strip()
            if not s:
                continue
            ids = [TOK_BOS] + encode_mixed_line(s, cjk_lut_map, trie)
            for tid in ids:
                tok_counts[tid] += 1
            buf.extend(struct.pack(f"<{len(ids)}H", *ids))
            if len(buf) >= 4 * 1024 * 1024:
                out_f.write(buf)
                total_tokens += len(buf) // 2
                buf.clear()
        if buf:
            out_f.write(buf)
            total_tokens += len(buf) // 2

    np.save(directory / "lut.npy", cjk_lut_arr)
    scale = float(tok_counts.sum())

    labels = {
        "format": "WBLM0003",
        "vocab": vocab_total,
        "cjk_vocab": len(vocab_cjk),
        "ext_base_id": EXT_BASE_ID,
        "ext_vocab": ext_vocab,
        "lines": total_lines,
        "tokens": total_tokens,
        "uni_logp": np.log(tok_counts / scale).tolist(),
    }
    (directory / "vocab.json").write_text(json.dumps(labels, ensure_ascii=False) + "\n", encoding="utf-8")
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
            try:
                for text in iter_records(group, path):
                    writer.add(group, text)
                    if cap is not None and writer.group_chars(group) >= cap:
                        break
            except Exception as exc:
                print(f"[corpus] WARN skipping unreadable shard {group}/{file}: {exc}", flush=True)
                continue
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
    manifest = {
        "format": "WBLM0002",
        "sources": use,
        "groups": {k: dict(v) for k, v in stats.items()},
        "tokens": result["tokens"],
        "vocab": result["vocab"],
        "prepare_seconds": round(time.monotonic() - started),
    }
    (directory / "corpus.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    return result


# ---------------------------------------------------------------------------
# Lightweight Local Streaming Audit (verifies mixed CJK+English vocab & Trie)
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
    all_norm_lines: list[str] = []
    ascii_words: Counter[str] = Counter()

    def collect(name: str, is_tech: bool, is_en: bool, raw_iter, max_docs: int = 200):
        cnt = 0
        chars = 0
        samples = []
        for doc in raw_iter:
            if cnt >= max_docs:
                break
            if not doc or any(kw in doc for kw in SPAM_KW):
                continue
            cl = clean_text(doc, is_tech=is_tech, is_english=is_en)
            norm = normalize_mixed_text(cl)
            if len(norm) < 8:
                continue
            cnt += 1
            chars += len(norm)
            all_norm_lines.append(norm)
            for m in RE_ASCII_WORD.finditer(norm):
                ascii_words[m.group(0)] += 1
            if len(samples) < 2:
                samples.append(norm[:110])
        print(f"[audit] {name:20s}: docs={cnt:3d}, chars={chars:7,d} | ex: {samples[0] if samples else ''}", flush=True)

    b = http_range_bytes(
        "https://huggingface.co/datasets/Orphanage/Baidu_Tieba_SunXiaochuan/resolve/main/No_train_validation_split.jsonl",
        800_000,
    )
    collect("tieba_sxc", False, False, parse_tieba_sxc_jsonl(b.decode("utf-8", errors="ignore").splitlines()[:-1]), 200)

    b = http_range_bytes(
        "https://huggingface.co/datasets/liwu/MNBVC/resolve/main/blog/ai_blog/20230923/000.jsonl.gz",
        1_200_000,
    )
    p = subprocess.run(["gzip", "-dc"], input=b, capture_output=True)
    collect("mnbvc_ai_blog", True, False, parse_mnbvc_blog_jsonl(p.stdout.decode("utf-8", errors="ignore").splitlines()[:-1]), 120)

    try:
        data = http_get_json(
            "https://datasets-server.huggingface.co/rows?dataset=BAAI/IndustryCorpus2_computer_programming_code&config=default&split=train&offset=0&length=80"
        )
        collect("ic2_zh_code", True, False, [r["row"].get("text", "") for r in data.get("rows", [])], 80)
    except Exception as e:
        print(f"[audit] ic2_zh_code: SKIP ({e})", flush=True)

    ext_vocab = build_ascii_subword_vocab(ascii_words, target_size=EXT_VOCAB_SIZE)
    trie = AsciiTrie(EXT_BASE_ID, ext_vocab)
    test_spans = [
        b"Rust", b"Python", b"iPhone 17 Pro", b"v2.1", b"asyncio", b"JavaScript",
        b"TensorFlow", b"Kubernetes", b"399.9", b"Git PR",
    ]
    total_bytes = sum(len(s) for s in test_spans)
    total_toks = 0
    unks = 0
    print(f"\n[vocab] built ext_vocab={len(ext_vocab)} (top multi-char: {ext_vocab[95:115]})")
    for sp in test_spans:
        ids = trie.encode_ascii(sp)
        total_toks += len(ids)
        unks += sum(1 for t in ids if t == TOK_UNK)
        pieces = [ext_vocab[t - EXT_BASE_ID] for t in ids]
        print(f"  {sp.decode():16s} -> {len(ids)} toks: {pieces}")
    print(f"[compression] {total_bytes} ASCII bytes -> {total_toks} tokens ({total_bytes/max(1,total_toks):.2f} chars/tok), UNKs={unks}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--sample-audit", action="store_true")
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--seed-dir")
    args = ap.parse_args()
    if args.sample_audit:
        run_sample_audit(Path(args.out_dir))
    else:
        if not args.seed_dir:
            ap.error("--seed-dir is required for full corpus build")
        prepare(Path(args.out_dir), Path(args.seed_dir))


if __name__ == "__main__":
    main()
