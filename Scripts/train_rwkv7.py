#!/usr/bin/env python3
"""
End-to-end Cloud GPU corpus builder + Micro-RWKV-7 trainer + QAT & Wubi-neighborhood
fine-tuner + WBLM0001 INT8 binary exporter for Formordo (形序).
"""
from __future__ import annotations

import glob
import gzip
import hashlib
import io
import json
import math
import os
import random
import re
import struct
import subprocess
import sys
import tarfile
import time
import urllib.request
import zipfile
from collections import Counter
from pathlib import Path

# Ensure fast C/Rust extensions are installed on Cloud GPU VMs
def ensure_deps(smoke: bool = False):
    pkgs = [("zstandard", "zstandard"), ("opencc-python-reimplemented", "opencc"), ("pyarrow", "pyarrow")]
    if not smoke:
        pkgs.extend([("huggingface_hub", "huggingface_hub"), ("ninja", "ninja")])
    need = []
    for pkg, mod in pkgs:
        try:
            __import__(mod)
        except ImportError:
            need.append(pkg)
    if need:
        print(f"[setup] Installing {need}...", flush=True)
        subprocess.check_call([sys.executable, "-m", "pip", "install", "-q", *need])

ensure_deps("--smoke" in sys.argv)

import numpy as np
import pyarrow.parquet as pq
import torch
import torch.nn as nn
import torch.nn.functional as F
import zstandard as zstd
from opencc import OpenCC

WBLM_MAGIC = b"WBLM0001"
CJK_LUT_BASE = 0x3400
CJK_LUT_LEN = 0xA000 - 0x3400
HEAD_DIM = 32

RE_URL = re.compile(r"https?://\S+|www\.\S+")
RE_MD_CODE = re.compile(r"```.*?```", re.DOTALL)
RE_MD_INLINE = re.compile(r"`[^`]{1,120}`")
RE_MD_IMG = re.compile(r"!\[[^\]]*\]\([^)]*\)")
RE_MD_LINK = re.compile(r"\[([^\]]{1,80})\]\([^)]*\)")
RE_HTML = re.compile(r"<[^>]{1,120}>")
RE_XHS_EMOJI = re.compile(r"\[[^\]]{1,10}[Rr]\]")
RE_XHS_TOPIC = re.compile(r"#[^#\n]{1,30}(?:\[话题\]#?|#)")
RE_MENTION = re.compile(r"@[\w\u4e00-\u9fff\-_.]{1,24}")
RE_TIEBA_UI = re.compile(
    r"我也说一句(?:还有\d*条回复)?|还有\d+条回复|点击查看(?:全部)?|收起回复|"
    r"来自(?:Android|iPhone|iOS|手机|网页)?客户端|贴吧用户[_A-Za-z0-9]*|"
    r"回复\s*[\w\u4e00-\u9fff\-_.]{1,20}\s*[:：]"
)
RE_WIKI_STUB = re.compile(
    r"的一个市镇|的一个街区|的一个村|的一个社区|邮政编码|INSEE编码|"
    r"位于法国|位于意大利|位于西班牙|位于德国|位于美国[^。\n]{0,20}县|"
    r"是[^。\n]{1,20}科[^。\n]{1,20}属的|小行星|巡天系统|绕太阳运转|轨道倾角|的一座火车站|"
    r"的居民点|的自治市镇|的镇区|的市镇"
)
RE_INTER_CJK_SPACE = re.compile(r"(?<=[\u4e00-\u9fff])[ \t]+(?=[\u4e00-\u9fff])")
RE_CJK_RUN = re.compile(r"[\u4e00-\u9fff]{2,}")

SPAM_KW = (
    "购物礼金", "优惠礼包", "京东首页搜", "淘宝搜", "加微信", "私信领",
    "扫码领", "点我头像", "点击下方链接", "复制这条信息", "打开淘宝", "领券下单",
)
WIKI_CUTS = ("\n参考文献\n", "\n外部链接\n", "\n参见\n", "\n参考资料\n", "\n外部连结\n")


def clean_text(text: str, cc: OpenCC | None = None) -> str:
    if not text:
        return ""
    text = RE_MD_CODE.sub(" ", text)
    text = RE_MD_IMG.sub(" ", text)
    text = RE_MD_LINK.sub(r"\1", text)
    text = RE_URL.sub(" ", text)
    text = RE_HTML.sub(" ", text)
    text = RE_MD_INLINE.sub(" ", text)
    text = RE_XHS_EMOJI.sub(" ", text)
    text = RE_XHS_TOPIC.sub(" ", text)
    text = RE_MENTION.sub(" ", text)
    text = RE_TIEBA_UI.sub(" ", text)
    if cc is not None:
        text = cc.convert(text)
    return text


def build_eval_leak_shield(seed_dir: Path) -> set[str]:
    """Collect all 10-char CJK substrings from val, test, and slang_2025_2026 to guarantee 0% train leakage."""
    grams: set[str] = set()
    for fname in ["val_sentences.tsv", "test_sentences.tsv", "slang_2025_2026.tsv"]:
        p = seed_dir / fname
        if not p.exists():
            continue
        for line in p.read_text(encoding="utf-8").splitlines():
            if not line or line.startswith("#") or "\t" not in line:
                continue
            sent = "".join(ch for ch in line.split("\t", 1)[1] if "\u4e00" <= ch <= "\u9fff")
            if len(sent) >= 10:
                for i in range(0, len(sent) - 9, 2):
                    grams.add(sent[i : i + 10])
            elif len(sent) >= 6:
                grams.add(sent)
    print(f"[shield] Loaded {len(grams):,} forbidden 10-gram substrings from held-out eval sets", flush=True)
    return grams


def has_eval_leak(s: str, shield: set[str]) -> bool:
    if len(s) < 10:
        return s in shield
    for i in range(0, len(s) - 9, 3):
        if s[i : i + 10] in shield:
            return True
    return s[-10:] in shield


def parse_wubi86_dict(yaml_path: Path):
    """Return char_first_key (cp -> 0..24), char_codes, and code_to_words for Wubi confusion-set mining."""
    char_first_key: dict[int, int] = {}
    code_to_words: dict[str, list[tuple[str, int]]] = {}
    word_to_code: dict[str, str] = {}
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
        if not text or not code or any(not ("a" <= c <= "y") for c in code):
            continue
        wt = int(parts[2].strip()) if len(parts) >= 3 and parts[2].strip().isdigit() else 100
        code_to_words.setdefault(code, []).append((text, wt))
        if len(text) == 1:
            cp = ord(text)
            if cp not in char_first_key:
                char_first_key[cp] = ord(code[0]) - ord("a")
        if text not in word_to_code or len(code) > len(word_to_code[text]):
            word_to_code[text] = code
    for c in code_to_words:
        code_to_words[c].sort(key=lambda x: -x[1])
    return char_first_key, code_to_words, word_to_code


def download_and_prepare_corpus(work_dir: Path, seed_dir: Path):
    from huggingface_hub import hf_hub_download
    clean_dir = work_dir / "clean"
    clean_dir.mkdir(parents=True, exist_ok=True)
    marker = clean_dir / "DONE.json"
    if marker.exists():
        print("[corpus] Found cached cleaned corpus:", marker.read_text(), flush=True)
        return json.loads(marker.read_text())

    cc = OpenCC("t2s")
    shield = build_eval_leak_shield(seed_dir)
    seen_sigs: set[bytes] = set()

    files_out = {
        "formal": open(clean_dir / "formal.txt", "w", encoding="utf-8"),
        "community": open(clean_dir / "community.txt", "w", encoding="utf-8"),
        "social": open(clean_dir / "social.txt", "w", encoding="utf-8"),
    }
    counts = {"formal": 0, "community": 0, "social": 0}

    def emit_doc(group: str, text: str) -> int:
        if not text:
            return 0
        for kw in SPAM_KW:
            if kw in text:
                return 0
        runs = RE_CJK_RUN.findall(text)
        if not runs:
            return 0
        skel = "".join(runs)
        if len(skel) < 4:
            return 0
        if has_eval_leak(skel, shield):
            return 0
        sig = hashlib.blake2b(f"{len(skel)}:{skel[:384]}".encode("utf-8"), digest_size=10).digest()
        if sig in seen_sigs:
            return 0
        seen_sigs.add(sig)
        out_f = files_out[group]
        # Write each document's runs separated by BOS boundary '\n', keeping intra-sentence clauses separated by spaces
        # Wait: in Wubi engine, context resets on punctuation! Let's write one clause/run per line so BOS aligns 1:1 with WubiEngine's punctuation context reset!
        c_sum = 0
        for r in runs:
            if len(r) >= 2:
                out_f.write(r + "\n")
                c_sum += len(r)
        counts[group] += c_sum
        return c_sum

    t0 = time.time()

    # 1. Everyday colloquial dialogue: silver/lccc (~95M chars) + Bilibili comments (~12M chars)
    print("[corpus 1/4] Downloading & cleaning LCCC everyday multi-turn chat...", flush=True)
    lccc_path = hf_hub_download(repo_id="silver/lccc", filename="lccc_base_train.jsonl.gz", repo_type="dataset")
    with gzip.open(lccc_path, "rt", encoding="utf-8", errors="ignore") as gz:
        for line in gz:
            try:
                turns = json.loads(line)
            except Exception:
                continue
            if not isinstance(turns, list):
                continue
            cleaned_turns = [RE_INTER_CJK_SPACE.sub("", t) for t in turns if isinstance(t, str)]
            joined = "，".join(cleaned_turns)
            emit_doc("social", joined)
            if counts["social"] >= 95_000_000:
                break
    try:
        os.remove(lccc_path)
    except OSError:
        pass
    print(f"  -> LCCC social chars: {counts['social']:,} ({time.time()-t0:.1f}s)", flush=True)

    print("[corpus 1b/4] Downloading & cleaning Bilibili comments...", flush=True)
    try:
        bili_path = hf_hub_download(repo_id="theblackcat102/bilibili_comments_sharegpt", filename="train.jsonl", repo_type="dataset")
        with open(bili_path, "r", encoding="utf-8", errors="ignore") as bf:
            for line in bf:
                try:
                    obj = json.loads(line)
                except Exception:
                    continue
                convs = obj.get("conversations") or []
                txt = "\n".join(c.get("value", "") for c in convs if isinstance(c, dict))
                emit_doc("social", clean_text(txt, cc))
                if counts["social"] >= 115_000_000:
                    break
    except Exception as e:
        print(f"  [warn] bilibili optional skip: {e}", flush=True)

    # Also include Tieba 2025 + RedNote notes (~3.8M recent slang/lifestyle)
    try:
        rnote = hf_hub_download(repo_id="FreshCrawl/rednote-xiaohongshu-notes", filename="notes.csv", repo_type="dataset")
    except Exception:
        rnote = None
    if rnote and os.path.exists(rnote):
        import csv
        with open(rnote, "r", encoding="utf-8", errors="ignore") as cf:
            for row in csv.DictReader(cf):
                txt = f"{row.get('title','')}\n{row.get('desc','') or row.get('content','')}"
                emit_doc("social", clean_text(txt, cc))
    print(f"  -> Total social/colloquial chars: {counts['social']:,}", flush=True)

    # 2. Modern 2024-2026 Zhihu Q&A (~115M chars)
    print("[corpus 2/4] Downloading & cleaning 2024-2026 Zhihu Q&A shards...", flush=True)
    min_ts = 1717200000  # 2024-06-01 UTC (recent 2.5 years)
    for shard_idx in [93, 92, 91, 90, 89, 88, 87, 86]:
        if counts["community"] >= 115_000_000:
            break
        sname = f"shards/part-{shard_idx:06d}.jsonl.zst"
        try:
            sp = hf_hub_download(repo_id="KrisTHL181/zhihu-qa", filename=sname, repo_type="dataset")
        except Exception as e:
            print(f"  [warn] skip {sname}: {e}", flush=True)
            continue
        with open(sp, "rb") as sf:
            dctx = zstd.ZstdDecompressor()
            with dctx.stream_reader(sf) as reader:
                tstream = io.TextIOWrapper(reader, encoding="utf-8", errors="ignore")
                for line in tstream:
                    try:
                        obj = json.loads(line)
                    except Exception:
                        continue
                    if (obj.get("created_time") or 0) < min_ts:
                        continue
                    q = obj.get("question_title") or ""
                    if q == "untitled":
                        q = ""
                    body = obj.get("content") or ""
                    emit_doc("community", clean_text(f"{q}\n{body}", cc))
                    if counts["community"] >= 115_000_000:
                        break
        try:
            os.remove(sp)
        except OSError:
            pass
        print(f"  -> after {sname}: community={counts['community']:,} ({time.time()-t0:.1f}s)", flush=True)

    # 3. Formal & Modern Articles: Fineweb-Edu-Chinese 4_5 (~75M) + zhwiki (~35M) = ~110M chars
    print("[corpus 3/4] Downloading & cleaning Fineweb-Edu-Chinese 4_5 & Wikipedia...", flush=True)
    for idx in range(24):
        if counts["formal"] >= 75_000_000:
            break
        fname = f"4_5/{idx:06d}.parquet"
        try:
            fp = hf_hub_download(repo_id="opencsg/Fineweb-Edu-Chinese-V2.1", filename=fname, repo_type="dataset")
        except Exception as e:
            print(f"  [warn] skip {fname}: {e}", flush=True)
            continue
        pf = pq.ParquetFile(fp)
        for rg in range(pf.num_row_groups):
            tbl = pf.read_row_group(rg, columns=["text"])
            for txt in tbl["text"].to_pylist():
                if txt and len(txt) >= 120:
                    emit_doc("formal", clean_text(txt, cc))
                    if counts["formal"] >= 75_000_000:
                        break
            if counts["formal"] >= 75_000_000:
                break
        try:
            os.remove(fp)
        except OSError:
            pass
    print(f"  -> after Fineweb: formal={counts['formal']:,}", flush=True)

    for wshard in ["20231101.zh/train-00002-of-00006.parquet", "20231101.zh/train-00001-of-00006.parquet"]:
        if counts["formal"] >= 110_000_000:
            break
        try:
            wp = hf_hub_download(repo_id="wikimedia/wikipedia", filename=wshard, repo_type="dataset")
        except Exception as e:
            print(f"  [warn] skip {wshard}: {e}", flush=True)
            continue
        pf = pq.ParquetFile(wp)
        for rg in range(pf.num_row_groups):
            tbl = pf.read_row_group(rg, columns=["title", "text"])
            for title, body in zip(tbl["title"].to_pylist(), tbl["text"].to_pylist()):
                if not body or len(body) < 500:
                    continue
                for cut in WIKI_CUTS:
                    pos = body.find(cut)
                    if pos != -1:
                        body = body[:pos]
                cleaned = clean_text(f"{title or ''}\n{body}", cc)
                if RE_WIKI_STUB.search(cleaned[:320]):
                    continue
                emit_doc("formal", cleaned)
                if counts["formal"] >= 110_000_000:
                    break
            if counts["formal"] >= 110_000_000:
                break
        try:
            os.remove(wp)
        except OSError:
            pass

    for f in files_out.values():
        f.close()

    counts["total"] = counts["formal"] + counts["community"] + counts["social"]
    marker.write_text(json.dumps(counts, indent=2))
    print(f"[corpus DONE] {counts} in {time.time()-t0:.1f}s", flush=True)
    return counts


def build_vocab_and_token_bin(clean_dir: Path, out_dir: Path, char_first_key: dict[int, int], vocab_size: int = 8192):
    """Build 8,192 char vocab + compact N-gram TSVs + shuffled uint16 binary token stream."""
    tok_bin = out_dir / "train_tokens.bin"
    meta_json = out_dir / "vocab_meta.json"
    if tok_bin.exists() and meta_json.exists():
        print("[pack] Reusing existing packed token stream", flush=True)
        meta = json.loads(meta_json.read_text(encoding="utf-8"))
        return meta, tok_bin

    t0 = time.time()
    uni_cnt = Counter()
    bi_cnt = Counter()
    group_files = [
        ("formal", clean_dir / "formal.txt", 0.34),
        ("community", clean_dir / "community.txt", 0.33),
        ("social", clean_dir / "social.txt", 0.33),
    ]

    print("[pack 1/3] Counting character unigrams & bigrams across 300M+ corpus...", flush=True)
    for gname, gpath, _ in group_files:
        if not gpath.exists():
            continue
        with open(gpath, "r", encoding="utf-8") as f:
            for line in f:
                s = line.strip()
                if len(s) < 2:
                    continue
                uni_cnt.update(s)
                for i in range(len(s) - 1):
                    bi_cnt[(s[i], s[i + 1])] += 1
        print(f"  counted {gname}: unique chars={len(uni_cnt):,}, bigrams={len(bi_cnt):,}", flush=True)

    # Ensure common Wubi single chars are included alongside top corpus chars
    scored_chars = []
    for ch, c in uni_cnt.items():
        cp = ord(ch)
        if CJK_LUT_BASE <= cp < 0xA000:
            bonus = 50 if cp in char_first_key else 0
            scored_chars.append((c + bonus, cp, ch))
    for cp in char_first_key:
        if CJK_LUT_BASE <= cp < 0xA000 and chr(cp) not in uni_cnt:
            scored_chars.append((1, cp, chr(cp)))

    scored_chars.sort(key=lambda x: (-x[0], x[1]))
    kept = scored_chars[: vocab_size - 2]
    kept.sort(key=lambda x: x[1])  # Sort by codepoint for clean deterministic IDs 2..8191

    lut = np.ones(CJK_LUT_LEN, dtype=np.uint16)  # default 1 = <UNK>
    tok_to_cp = [0, 1]
    total_chars = sum(uni_cnt.values())
    uni_logp = np.full(vocab_size, -13.5, dtype=np.float32)
    uni_logp[0] = -16.0  # <BOS>
    unk_mass = max(total_chars - sum(uni_cnt[ch] for _, _, ch in kept), 100)
    uni_logp[1] = math.log(unk_mass / (total_chars + 1.0))

    # Write unigrams.tsv (12,000 entries for WBAUX003 lexical prior)
    out_dir.mkdir(parents=True, exist_ok=True)
    with open(out_dir / "unigrams.tsv", "w", encoding="utf-8") as uf:
        for ch, c in uni_cnt.most_common(12000):
            if c >= 5 and CJK_LUT_BASE <= ord(ch) < 0xA000:
                p = (c + 0.5) / (total_chars + 4000.0)
                sc = max(0.0, min(14.0, math.log(p) + 14.0))
                uf.write(f"{ch}\t{sc:.4f}\n")

    for idx, (_, cp, ch) in enumerate(kept):
        tid = idx + 2
        lut[cp - CJK_LUT_BASE] = tid
        tok_to_cp.append(cp)
        c = uni_cnt[ch]
        uni_logp[tid] = math.log((c + 1.0) / (total_chars + vocab_size))

    # Write compact high-PMI bigrams.tsv (top 80,000) & trigrams.tsv (top 35,000) for hybrid WBAUX003
    print("[pack 2/3] Exporting compact high-PMI bigrams (80k) & trigrams (35k)...", flush=True)
    scored_bi = []
    p_bi_keep = {}
    alpha_bi = 50.0
    for (c1, c2), bm in bi_cnt.items():
        if bm < 8:
            continue
        lm = uni_cnt[c1]
        pu2 = (uni_cnt[c2] + 1.0) / total_chars
        p_bi = (bm + alpha_bi * pu2) / (lm + alpha_bi)
        pmi = math.log(p_bi / pu2)
        if bm >= 20:
            p_bi_keep[(c1, c2)] = p_bi
        if pmi >= 0.25:
            boost = min(5.5, pmi)
            gain = (bm ** 0.82) * boost
            scored_bi.append((gain, c1, c2, boost))
    scored_bi.sort(key=lambda x: -x[0])
    with open(out_dir / "bigrams.tsv", "w", encoding="utf-8") as bf:
        for _, c1, c2, boost in scored_bi[:80000]:
            bf.write(f"{c1}{c2}\t{boost:.4f}\n")
    del scored_bi

    # Fast pruned trigram pass + collect lines for shuffled binary packing
    tri_cnt = Counter()
    all_lines: list[str] = []
    for gname, gpath, _ in group_files:
        if not gpath.exists():
            continue
        with open(gpath, "r", encoding="utf-8") as f:
            for line in f:
                s = line.strip()
                if len(s) < 2:
                    continue
                all_lines.append(s)
                if len(s) >= 3:
                    for i in range(len(s) - 2):
                        a, b, c = s[i], s[i + 1], s[i + 2]
                        if (a, b) in p_bi_keep and (b, c) in p_bi_keep:
                            tri_cnt[(a, b, c)] += 1
        # Keep memory lean
        if len(tri_cnt) > 6_000_000:
            tri_cnt = Counter(dict(tri_cnt.most_common(3_000_000)))

    scored_tri = []
    beta_tri = 25.0
    for (c0, c1, c2), tm in tri_cnt.items():
        if tm < 12:
            continue
        c01 = bi_cnt.get((c0, c1), 1)
        p_bi_12 = p_bi_keep.get((c1, c2), (uni_cnt[c2] + 1.0) / total_chars)
        p_tri = (tm + beta_tri * p_bi_12) / (c01 + beta_tri)
        lift = math.log(p_tri / p_bi_12)
        if lift >= 0.45:
            boost = min(5.0, lift)
            scored_tri.append(((tm ** 0.82) * boost, c0, c1, c2, boost))
    scored_tri.sort(key=lambda x: -x[0])
    with open(out_dir / "trigrams.tsv", "w", encoding="utf-8") as tf:
        for _, c0, c1, c2, boost in scored_tri[:35000]:
            tf.write(f"{c0}{c1}{c2}\t{boost:.4f}\n")
    del tri_cnt, scored_tri, bi_cnt, p_bi_keep

    # Shuffle clauses globally so every batch mixes formal + Zhihu + colloquial chat evenly!
    print(f"[pack 3/3] Shuffling {len(all_lines):,} clauses & writing uint16 token stream...", flush=True)
    rng = random.Random(20261119)
    rng.shuffle(all_lines)

    # Encode each clause prefixed by 0 (<BOS>)!
    # Wait: in NeuralContextTracker, <BOS> (token 0) resets/starts each clause cleanly!
    total_toks = 0
    with open(tok_bin, "wb") as bf:
        chunk_arr = bytearray()
        for s in all_lines:
            chunk_arr.extend(b"\x00\x00")  # <BOS> = 0
            for ch in s:
                cp = ord(ch)
                tid = int(lut[cp - CJK_LUT_BASE]) if CJK_LUT_BASE <= cp < 0xA000 else 1
                chunk_arr.extend(struct.pack("<H", tid))
            if len(chunk_arr) >= 4 * 1024 * 1024:
                bf.write(chunk_arr)
                total_toks += len(chunk_arr) // 2
                chunk_arr.clear()
        if chunk_arr:
            bf.write(chunk_arr)
            total_toks += len(chunk_arr) // 2
    del all_lines

    # Map token_id -> first Wubi key (0..24, or 255 if unknown)
    tok_first_key = [255, 255]
    for cp in tok_to_cp[2:]:
        tok_first_key.append(char_first_key.get(cp, 255))

    meta = {
        "vocab_size": vocab_size,
        "total_tokens": total_toks,
        "tok_to_cp": tok_to_cp,
        "tok_first_key": tok_first_key,
        "uni_logp": uni_logp.tolist(),
    }
    np.save(out_dir / "lut.npy", lut)
    meta_json.write_text(json.dumps(meta))
    print(f"[pack DONE] {total_toks:,} tokens written in {time.time()-t0:.1f}s", flush=True)
    return meta, tok_bin


# ==============================================================================
# Custom CUDA Warp-Register Kernel for RWKV-7 Gated Delta-Rule Scan + BOS Reset
# ==============================================================================
CUDA_SRC = r"""
#include <torch/extension.h>
#include <cuda.h>
#include <cuda_runtime.h>

// Each CUDA thread handles ONE independent row `i` (0..31) of ONE `(b, h)` head state `S[32][32]`!
// Total threads = B * H * 32. Within each block of 32 threads (1 warp = 1 `(b, h)`),
// threads share `r, k_hat, w_dec, reset` across `j=0..31` in shared memory / warp shuffles,
// and hold their own row `s[0..31]` strictly in 32 float registers!
// When `bos_mask[b, t] == 1` (token == 0 <BOS>), state `s[0..31]` resets to 0 BEFORE step `t`
// so clauses packed into a contiguous 256-token window NEVER leak across <BOS> boundaries!

#define HD 32
#define CKP 16

__global__ void rwkv7_delta_fwd_kernel(
    const float* __restrict__ r,       // [B, T, H, 32]
    const float* __restrict__ k_hat,   // [B, T, H, 32]
    const float* __restrict__ v,       // [B, T, H, 32]
    const float* __restrict__ w_dec,   // [B, T, H, 32]
    const float* __restrict__ beta,    // [B, T, H, 32]
    const uint8_t* __restrict__ bos,   // [B, T] (1 if tok_t == 0 else 0)
    float* __restrict__ y,             // [B, T, H, 32]
    float* __restrict__ s_ckp,         // [B, num_ckp, H, 32, 32]
    int B, int T, int H, int num_ckp
) {
    int bh = blockIdx.x; // 0 .. B*H - 1
    int i = threadIdx.x; // 0 .. 31 (row index AND warp lane!)
    int b = bh / H;
    int h = bh % H;

    float s[HD];
    #pragma unroll
    for (int j = 0; j < HD; ++j) s[j] = 0.0f;

    __shared__ float sh_r[HD];
    __shared__ float sh_k[HD];
    __shared__ float sh_w[HD];

    for (int t = 0; t < T; ++t) {
        if (t % CKP == 0) {
            int c_idx = t / CKP;
            int base_ckp = (((b * num_ckp + c_idx) * H + h) * HD + i) * HD;
            #pragma unroll
            for (int j = 0; j < HD; ++j) {
                s_ckp[base_ckp + j] = s[j];
            }
        }
        if (bos[b * T + t]) {
            #pragma unroll
            for (int j = 0; j < HD; ++j) s[j] = 0.0f;
        }
        int off = ((b * T + t) * H + h) * HD;
        sh_r[i] = r[off + i];
        sh_k[i] = k_hat[off + i];
        sh_w[i] = w_dec[off + i];
        __syncwarp();

        float vi = v[off + i];
        float bi = beta[off + i];

        float v_pred = 0.0f;
        #pragma unroll
        for (int j = 0; j < HD; ++j) {
            float sj = s[j] * sh_w[j];
            s[j] = sj;
            v_pred += sj * sh_k[j];
        }
        float delta = bi * (vi - v_pred);
        float out = 0.0f;
        #pragma unroll
        for (int j = 0; j < HD; ++j) {
            float ns = s[j] + delta * sh_k[j];
            s[j] = ns;
            out += ns * sh_r[j];
        }
        y[off + i] = out;
        __syncwarp();
    }
}

__global__ void rwkv7_delta_bwd_kernel(
    const float* __restrict__ r,
    const float* __restrict__ k_hat,
    const float* __restrict__ v,
    const float* __restrict__ w_dec,
    const float* __restrict__ beta,
    const uint8_t* __restrict__ bos,
    const float* __restrict__ s_ckp,
    const float* __restrict__ dy,      // [B, T, H, 32]
    float* __restrict__ dr,
    float* __restrict__ dk_hat,
    float* __restrict__ dv,
    float* __restrict__ dw_dec,
    float* __restrict__ dbeta,
    int B, int T, int H, int num_ckp
) {
    int bh = blockIdx.x;
    int i = threadIdx.x; // 0..31
    int b = bh / H;
    int h = bh % H;

    float ds[HD];
    #pragma unroll
    for (int j = 0; j < HD; ++j) ds[j] = 0.0f;

    __shared__ float sh_r[CKP][HD];
    __shared__ float sh_k[CKP][HD];
    __shared__ float sh_w[CKP][HD];

    for (int c_idx = num_ckp - 1; c_idx >= 0; --c_idx) {
        int t_start = c_idx * CKP;
        int len = min(CKP, T - t_start);

        // Reload checkpoint state at `t_start` and recompute forward states inside this 16-step chunk
        float s_hist[CKP][HD];
        float s_cur[HD];
        int base_ckp = (((b * num_ckp + c_idx) * H + h) * HD + i) * HD;
        #pragma unroll
        for (int j = 0; j < HD; ++j) s_cur[j] = s_ckp[base_ckp + j];

        for (int lt = 0; lt < len; ++lt) {
            int t = t_start + lt;
            if (bos[b * T + t]) {
                #pragma unroll
                for (int j = 0; j < HD; ++j) s_cur[j] = 0.0f;
            }
            #pragma unroll
            for (int j = 0; j < HD; ++j) s_hist[lt][j] = s_cur[j]; // state BEFORE step `t` (after bos reset)

            int off = ((b * T + t) * H + h) * HD;
            sh_r[lt][i] = r[off + i];
            sh_k[lt][i] = k_hat[off + i];
            sh_w[lt][i] = w_dec[off + i];
            __syncwarp();

            float vi = v[off + i];
            float bi = beta[off + i];
            float vp = 0.0f;
            #pragma unroll
            for (int j = 0; j < HD; ++j) {
                s_cur[j] *= sh_w[lt][j];
                vp += s_cur[j] * sh_k[lt][j];
            }
            float dl = bi * (vi - vp);
            #pragma unroll
            for (int j = 0; j < HD; ++j) {
                s_cur[j] += dl * sh_k[lt][j];
            }
            __syncwarp();
        }

        // Reverse scan through `lt = len-1 .. 0`
        for (int lt = len - 1; lt >= 0; --lt) {
            int t = t_start + lt;
            int off = ((b * T + t) * H + h) * HD;
            float dy_i = dy[off + i];
            float vi = v[off + i];
            float bi = beta[off + i];

            // Recompute decayed state `sd[j] = s_prev[j] * w[j]` and `v_pred`, `s_next[j]`
            float vp = 0.0f;
            float sd[HD];
            #pragma unroll
            for (int j = 0; j < HD; ++j) {
                sd[j] = s_hist[lt][j] * sh_w[lt][j];
                vp += sd[j] * sh_k[lt][j];
            }
            float diff = vi - vp;
            float delta = bi * diff;

            // `out_i = sum_j (sd[j] + delta * k[j]) * r[j]`
            // Accumulate `ds_next[j] = ds[j] + dy_i * r[j]` and `dr_contrib[j] = s_next[j] * dy_i`
            float d_delta = 0.0f;
            float dr_sum_i = 0.0f;
            float dk_sum_i = 0.0f;
            float dw_sum_i = 0.0f;

            float ds_next[HD];
            #pragma unroll
            for (int j = 0; j < HD; ++j) {
                float s_next_j = sd[j] + delta * sh_k[lt][j];
                float dsn = ds[j] + dy_i * sh_r[lt][j];
                ds_next[j] = dsn;
                d_delta += dsn * sh_k[lt][j];
            }

            float dbi = d_delta * diff;
            float dvi = d_delta * bi;
            float dvp = -d_delta * bi;

            dbeta[off + i] = dbi;
            dv[off + i] = dvi;

            // Now reduce per-column j gradients across warp lanes `i = 0..31`!
            #pragma unroll
            for (int j = 0; j < HD; ++j) {
                float s_next_j = sd[j] + delta * sh_k[lt][j];
                float d_sd_j = ds_next[j] + dvp * sh_k[lt][j];

                float c_dr = s_next_j * dy_i;
                float c_dk = ds_next[j] * delta + dvp * sd[j];
                float c_dw = d_sd_j * s_hist[lt][j];

                // Warp reduction across all 32 rows `i` for fixed column `j`
                #pragma unroll
                for (int mask = 16; mask > 0; mask >>= 1) {
                    c_dr += __shfl_xor_sync(0xffffffff, c_dr, mask);
                    c_dk += __shfl_xor_sync(0xffffffff, c_dk, mask);
                    c_dw += __shfl_xor_sync(0xffffffff, c_dw, mask);
                }
                if (i == j) {
                    dr_sum_i = c_dr;
                    dk_sum_i = c_dk;
                    dw_sum_i = c_dw;
                }
                ds[j] = bos[b * T + t] ? 0.0f : (d_sd_j * sh_w[lt][j]);
            }
            dr[off + i] = dr_sum_i;
            dk_hat[off + i] = dk_sum_i;
            dw_dec[off + i] = dw_sum_i;
            __syncwarp();
        }
    }
}

std::vector<torch::Tensor> rwkv7_fwd(
    torch::Tensor r, torch::Tensor k_hat, torch::Tensor v,
    torch::Tensor w_dec, torch::Tensor beta, torch::Tensor bos
) {
    int B = r.size(0), T = r.size(1), H = r.size(2);
    int num_ckp = (T + CKP - 1) / CKP;
    auto y = torch::empty_like(r);
    auto s_ckp = torch::empty({B, num_ckp, H, HD, HD}, r.options());
    rwkv7_delta_fwd_kernel<<<B * H, HD>>>(
        r.data_ptr<float>(), k_hat.data_ptr<float>(), v.data_ptr<float>(),
        w_dec.data_ptr<float>(), beta.data_ptr<float>(), bos.data_ptr<uint8_t>(),
        y.data_ptr<float>(), s_ckp.data_ptr<float>(), B, T, H, num_ckp
    );
    return {y, s_ckp};
}

std::vector<torch::Tensor> rwkv7_bwd(
    torch::Tensor r, torch::Tensor k_hat, torch::Tensor v,
    torch::Tensor w_dec, torch::Tensor beta, torch::Tensor bos,
    torch::Tensor s_ckp, torch::Tensor dy
) {
    int B = r.size(0), T = r.size(1), H = r.size(2);
    int num_ckp = s_ckp.size(1);
    auto dr = torch::empty_like(r);
    auto dk = torch::empty_like(k_hat);
    auto dv = torch::empty_like(v);
    auto dw = torch::empty_like(w_dec);
    auto db = torch::empty_like(beta);
    rwkv7_delta_bwd_kernel<<<B * H, HD>>>(
        r.data_ptr<float>(), k_hat.data_ptr<float>(), v.data_ptr<float>(),
        w_dec.data_ptr<float>(), beta.data_ptr<float>(), bos.data_ptr<uint8_t>(),
        s_ckp.data_ptr<float>(), dy.data_ptr<float>(),
        dr.data_ptr<float>(), dk.data_ptr<float>(), dv.data_ptr<float>(),
        dw.data_ptr<float>(), db.data_ptr<float>(), B, T, H, num_ckp
    );
    return {dr, dk, dv, dw, db};
}

PYBIND11_MODULE(TORCH_EXTENSION_NAME, m) {
    m.def("fwd", &rwkv7_fwd, "RWKV7 Delta Forward");
    m.def("bwd", &rwkv7_bwd, "RWKV7 Delta Backward");
}
"""

_CUDA_EXT = None

def get_cuda_ext():
    global _CUDA_EXT
    if _CUDA_EXT is not None:
        return _CUDA_EXT
    if not torch.cuda.is_available():
        return None
    try:
        from torch.utils.cpp_extension import load_inline
        print("[cuda] Compiling custom warp-register RWKV-7 Delta kernel...", flush=True)
        _CUDA_EXT = load_inline(
            name="rwkv7_warp_ext",
            cpp_sources="",
            cuda_sources=CUDA_SRC,
            extra_cuda_cflags=["-O3", "--use_fast_math"],
            verbose=False,
        )
        print("[cuda] Compiled rwkv7_warp_ext successfully!", flush=True)
        return _CUDA_EXT
    except Exception as e:
        print(f"[cuda] Fallback to PyTorch scan ({e})", flush=True)
        _CUDA_EXT = False
        return None


class Rwkv7DeltaScanFn(torch.autograd.Function):
    @staticmethod
    def forward(ctx, r, k_hat, v, w_dec, beta, bos):
        r_c = r.contiguous().float()
        k_c = k_hat.contiguous().float()
        v_c = v.contiguous().float()
        w_c = w_dec.contiguous().float()
        b_c = beta.contiguous().float()
        bos_c = bos.contiguous().to(torch.uint8)
        y, s_ckp = _CUDA_EXT.fwd(r_c, k_c, v_c, w_c, b_c, bos_c)
        ctx.save_for_backward(r_c, k_c, v_c, w_c, b_c, bos_c, s_ckp)
        return y.to(r.dtype)

    @staticmethod
    def backward(ctx, dy):
        r_c, k_c, v_c, w_c, b_c, bos_c, s_ckp = ctx.saved_tensors
        dr, dk, dv, dw, db = _CUDA_EXT.bwd(r_c, k_c, v_c, w_c, b_c, bos_c, s_ckp, dy.contiguous().float())
        return dr.to(dy.dtype), dk.to(dy.dtype), dv.to(dy.dtype), dw.to(dy.dtype), db.to(dy.dtype), None


def rwkv7_scan_ref(r, k_hat, v, w_dec, beta, bos):
    """Exact PyTorch reference scan (for CPU/MPS & CUDA grad verification)."""
    B, T, H, D = r.shape
    S = r.new_zeros((B, H, D, D), dtype=torch.float32)
    outs = []
    for t in range(T):
        rst = (1.0 - bos[:, t].float()).view(B, 1, 1, 1)
        S = S * rst
        rt = r[:, t].float()
        kt = k_hat[:, t].float()
        vt = v[:, t].float()
        wt = w_dec[:, t].float()
        bt = beta[:, t].float()
        S = S * wt.unsqueeze(2)
        vp = torch.einsum("bhij,bhj->bhi", S, kt)
        delta = bt * (vt - vp)
        S = S + delta.unsqueeze(3) * kt.unsqueeze(2)
        yt = torch.einsum("bhij,bhj->bhi", S, rt)
        outs.append(yt.to(r.dtype))
    return torch.stack(outs, dim=1)


# ==============================================================================
# Quantization-Aware STE Helpers (matching Core/src/neural.rs 1:1)
# ==============================================================================
def ste_quant_weight_rows(w: torch.Tensor) -> torch.Tensor:
    max_abs = w.detach().abs().amax(dim=-1, keepdim=True).clamp(min=1e-7)
    scale = max_abs / 127.0
    w_q = (w / scale).round().clamp(-127.0, 127.0) * scale
    return w + (w_q - w).detach()


def ste_quant_act(x: torch.Tensor) -> torch.Tensor:
    max_abs = x.detach().abs().amax(dim=-1, keepdim=True).clamp(min=1e-7)
    scale = max_abs / 127.0
    x_q = (x / scale).round().clamp(-127.0, 127.0) * scale
    return x + (x_q - x).detach()


def qlinear(x: torch.Tensor, w: torch.Tensor, b: torch.Tensor | None, qat: bool) -> torch.Tensor:
    if qat:
        x = ste_quant_act(x)
        w = ste_quant_weight_rows(w)
    return F.linear(x, w, b)


class Rwkv7Block(nn.Module):
    def __init__(self, d_model: int, n_heads: int, ffn_dim: int, layer_id: int, n_layers: int):
        super().__init__()
        self.d = d_model
        self.h = n_heads
        self.ln1 = nn.Parameter(torch.ones(d_model))
        ratio = layer_id / max(n_layers - 1, 1)
        self.mu_tm = nn.Parameter(torch.full((d_model,), 0.35 - 0.15 * ratio))
        self.tm_proj = nn.Linear(d_model, 6 * d_model, bias=True)
        self.rho = nn.Parameter(torch.zeros(d_model))
        self.tm_out = nn.Linear(d_model, d_model, bias=False)

        self.ln2 = nn.Parameter(torch.ones(d_model))
        self.mu_cm = nn.Parameter(torch.full((d_model,), 0.40 - 0.15 * ratio))
        self.cm_up = nn.Linear(d_model, ffn_dim, bias=True)
        self.cm_dn = nn.Linear(ffn_dim, d_model, bias=False)

        with torch.no_grad():
            nn.init.orthogonal_(self.tm_proj.weight, gain=0.5)
            self.tm_proj.bias.zero_()
            # Initialize decay bias (`3D..4D`) across multi-scale horizons (`sigmoid(1.2 .. 3.8) ≈ 0.77 .. 0.98`)
            for h in range(n_heads):
                base_decay = 1.1 + 2.7 * (h / max(n_heads - 1, 1))
                self.tm_proj.bias[3 * d_model + h * HEAD_DIM : 3 * d_model + (h + 1) * HEAD_DIM] = base_decay
            # Initialize delta rate bias (`4D..5D`) around `sigmoid(-0.8) ≈ 0.31`
            self.tm_proj.bias[4 * d_model : 5 * d_model] = -0.8
            # Initialize output gate bias (`5D..6D`) around `1.0` so initial gates are open
            self.tm_proj.bias[5 * d_model : 6 * d_model] = 1.0
            nn.init.zeros_(self.tm_out.weight)
            nn.init.orthogonal_(self.cm_up.weight, gain=0.5)
            self.cm_up.bias.zero_()
            nn.init.zeros_(self.cm_dn.weight)

    @staticmethod
    def rms_norm(x: torch.Tensor, gamma: torch.Tensor) -> torch.Tensor:
        ms = x.float().pow(2).mean(dim=-1, keepdim=True)
        return (x.float() * torch.rsqrt(ms + 1e-5)).to(x.dtype) * gamma

    @staticmethod
    def token_shift(u: torch.Tensor, mu: torch.Tensor, bos: torch.Tensor) -> torch.Tensor:
        # At positions where `bos[:, t] == 1`, `prev_tm` in Rust was initialized to `0.0`!
        u_prev = F.pad(u[:, :-1, :], (0, 0, 1, 0))
        u_prev = u_prev * (1.0 - bos.unsqueeze(-1).to(u.dtype))
        return u + mu * (u_prev - u)

    def forward(self, x: torch.Tensor, bos: torch.Tensor, qat: bool = False) -> torch.Tensor:
        B, T, D = x.shape
        u = self.rms_norm(x, self.ln1)
        s_tm = self.token_shift(u, self.mu_tm, bos)
        p6 = qlinear(s_tm, self.tm_proj.weight, self.tm_proj.bias, qat)

        r, k_raw, v, w_raw, b_raw, g = p6.chunk(6, dim=-1)
        r_h = r.view(B, T, self.h, HEAD_DIM)
        k_h = F.normalize(k_raw.float().view(B, T, self.h, HEAD_DIM), p=2.0, dim=-1, eps=1e-4).to(x.dtype)
        v_h = v.view(B, T, self.h, HEAD_DIM)
        w_dec = torch.sigmoid(w_raw.float().clamp(-20.0, 20.0)).view(B, T, self.h, HEAD_DIM)
        beta = torch.sigmoid(b_raw.float().clamp(-20.0, 20.0)).view(B, T, self.h, HEAD_DIM)

        ext = get_cuda_ext()
        if ext and x.is_cuda:
            y_scan = Rwkv7DeltaScanFn.apply(r_h, k_h, v_h, w_dec, beta, bos)
        else:
            y_scan = rwkv7_scan_ref(r_h, k_h, v_h, w_dec, beta, bos)

        y_flat = y_scan.reshape(B, T, D) + self.rho * r * v
        y_tm = y_flat * F.silu(g)
        x = x + qlinear(y_tm, self.tm_out.weight, None, qat)

        u2 = self.rms_norm(x, self.ln2)
        s_cm = self.token_shift(u2, self.mu_cm, bos)
        ffn = torch.relu(qlinear(s_cm, self.cm_up.weight, self.cm_up.bias, qat)).square()
        x = x + qlinear(ffn, self.cm_dn.weight, None, qat)
        return x


class MicroRwkv7Model(nn.Module):
    def __init__(self, vocab_size=8192, rank=128, d_model=192, n_heads=6, n_layers=3, ffn_dim=512, init_uni_logp=None):
        super().__init__()
        self.vocab_size = vocab_size
        self.rank = rank
        self.d_model = d_model
        self.n_heads = n_heads
        self.n_layers = n_layers
        self.ffn_dim = ffn_dim

        self.emb = nn.Parameter(torch.randn(vocab_size, rank) * 0.12)
        if init_uni_logp is not None:
            self.uni_bias = nn.Parameter(torch.tensor(init_uni_logp, dtype=torch.float32))
        else:
            self.uni_bias = nn.Parameter(torch.full((vocab_size,), -math.log(vocab_size)))

        self.w_up = nn.Linear(rank, d_model, bias=False)
        self.blocks = nn.ModuleList([
            Rwkv7Block(d_model, n_heads, ffn_dim, l, n_layers) for l in range(n_layers)
        ])
        self.ln_out = nn.Parameter(torch.ones(d_model))
        self.w_dn = nn.Linear(d_model, rank, bias=True)
        self.w_mtp_h = nn.Linear(d_model, rank, bias=True)
        self.w_mtp_c1 = nn.Linear(rank, rank, bias=False)

        with torch.no_grad():
            nn.init.orthogonal_(self.w_up.weight, gain=0.8)
            nn.init.orthogonal_(self.w_dn.weight, gain=0.4)
            self.w_dn.bias.zero_()
            nn.init.orthogonal_(self.w_mtp_h.weight, gain=0.4)
            self.w_mtp_h.bias.zero_()
            nn.init.orthogonal_(self.w_mtp_c1.weight, gain=0.4)

    def encode_hidden(self, tokens: torch.Tensor, qat: bool = False) -> torch.Tensor:
        bos = (tokens == 0)
        e_table = ste_quant_weight_rows(self.emb) if qat else self.emb
        e_tok = e_table[tokens]
        w_up = ste_quant_weight_rows(self.w_up.weight) if qat else self.w_up.weight
        x = F.linear(e_tok, w_up)
        for blk in self.blocks:
            x = blk(x, bos, qat=qat)
        h_out = Rwkv7Block.rms_norm(x, self.ln_out)
        if qat:
            h_out = ste_quant_act(h_out)
        return h_out

    def forward_train(self, seq: torch.Tensor, tok_first_key_t: torch.Tensor | None = None, qat: bool = False):
        # seq: [B, T + 2]
        inp = seq[:, :-2]   # [B, T]
        tgt1 = seq[:, 1:-1] # [B, T] (+1 next char; 0 means end-of-clause <BOS>, ignore!)
        tgt2 = seq[:, 2:]   # [B, T] (+2 char; valid when both tgt1 != 0 and tgt2 != 0)

        h = self.encode_hidden(inp, qat=qat)
        e_table = ste_quant_weight_rows(self.emb) if qat else self.emb

        p1 = F.linear(h, ste_quant_weight_rows(self.w_dn.weight) if qat else self.w_dn.weight, self.w_dn.bias)
        if qat:
            p1 = ste_quant_act(p1)
        logits1 = F.linear(p1, e_table, self.uni_bias)  # [B, T, V]

        valid1 = (tgt1 >= 2)  # ignore <BOS>=0 and <UNK>=1 targets
        ce1 = F.cross_entropy(logits1.view(-1, self.vocab_size), tgt1.reshape(-1), ignore_index=0)

        # Self-normalization penalty: drives `logsumexp(logits1) -> 0` so raw dot products are true log-probs!
        lse1 = torch.logsumexp(logits1.float(), dim=-1)
        sn_loss = (lse1[valid1].square().mean()) if valid1.any() else lse1.square().mean()

        # MTP-2 loss on valid 2-char continuations (subsample 1/2 positions to keep compute tiny)
        valid2 = (tgt1 >= 2) & (tgt2 >= 2)
        if valid2.any():
            h_v = h[valid2]
            c1_v = tgt1[valid2]
            c2_v = tgt2[valid2]
            if h_v.shape[0] > 8192:
                idx = torch.randperm(h_v.shape[0], device=h.device)[:8192]
                h_v, c1_v, c2_v = h_v[idx], c1_v[idx], c2_v[idx]
            mtp_h = F.linear(h_v, ste_quant_weight_rows(self.w_mtp_h.weight) if qat else self.w_mtp_h.weight, self.w_mtp_h.bias)
            mtp_c = F.linear(e_table[c1_v], ste_quant_weight_rows(self.w_mtp_c1.weight) if qat else self.w_mtp_c1.weight)
            p2 = mtp_h + mtp_c
            if qat:
                p2 = ste_quant_act(p2)
            logits2 = F.linear(p2, e_table, self.uni_bias)
            ce2 = F.cross_entropy(logits2, c2_v)
            lse2 = torch.logsumexp(logits2.float(), dim=-1).square().mean()
        else:
            ce2 = ce1 * 0.0
            lse2 = sn_loss * 0.0

        loss = ce1 + 0.35 * ce2 + 0.06 * (sn_loss + 0.5 * lse2)
        return loss, ce1.detach(), ce2.detach(), lse1.detach().abs().mean(), h, p1, e_table


def verify_cuda_kernel_grad():
    ext = get_cuda_ext()
    if not ext:
        return
    torch.manual_seed(0)
    B, T, H, D = 4, 37, 6, 32
    r = torch.randn(B, T, H, D, device="cuda", requires_grad=True)
    k = F.normalize(torch.randn(B, T, H, D, device="cuda"), dim=-1).detach().requires_grad_(True)
    v = torch.randn(B, T, H, D, device="cuda", requires_grad=True)
    w = torch.sigmoid(torch.randn(B, T, H, D, device="cuda") + 1.5).detach().requires_grad_(True)
    b = torch.sigmoid(torch.randn(B, T, H, D, device="cuda") - 0.5).detach().requires_grad_(True)
    bos = (torch.rand(B, T, device="cuda") < 0.12)

    y_cu = Rwkv7DeltaScanFn.apply(r, k, v, w, b, bos)
    g_out = torch.randn_like(y_cu)
    y_cu.backward(g_out)
    grads_cu = [t.grad.clone() for t in (r, k, v, w, b)]
    for t in (r, k, v, w, b):
        t.grad = None

    y_ref = rwkv7_scan_ref(r, k, v, w, b, bos)
    y_ref.backward(g_out)
    grads_ref = [t.grad.clone() for t in (r, k, v, w, b)]

    fwd_err = (y_cu - y_ref).abs().max().item()
    bwd_err = max((gc - gr).abs().max().item() for gc, gr in zip(grads_cu, grads_ref))
    print(f"[cuda check] fwd_max_err={fwd_err:.2e}, bwd_max_err={bwd_err:.2e}", flush=True)
    assert fwd_err < 1e-4 and bwd_err < 5e-4, f"CUDA kernel mismatch: {fwd_err}, {bwd_err}"


def build_wubi_confusable_table(meta: dict, code_to_words: dict, word_to_code: dict):
    """Build per-token confusable Wubi neighbor token table `[V, 8]` for Stage 2 discriminative margin tuning."""
    # QWERTY layout adjacency
    rows_q = ["qwertyuiop", "asdfghjkl", "zxcvbnm"]
    centers = {}
    for c_i, ch in enumerate(rows_q[0]): centers[ch] = (float(c_i), 0.0)
    for c_i, ch in enumerate(rows_q[1]): centers[ch] = (c_i + 0.5, 1.0)
    for c_i, ch in enumerate(rows_q[2]): centers[ch] = (c_i + 1.5, 2.0)

    neighbors = {}
    for ch in "abcdefghijklmnopqrstuvwxy":
        cx, cy = centers[ch]
        nbs = []
        for ch2 in "abcdefghijklmnopqrstuvwxy":
            if ch2 == ch: continue
            dx, dy = abs(centers[ch2][0] - cx), abs(centers[ch2][1] - cy)
            if (dy < 0.1 and dx <= 1.05) or (dy <= 1.05 and dx <= 0.85):
                nbs.append(ch2)
        neighbors[ch] = nbs

    cp_to_tok = {cp: tid for tid, cp in enumerate(meta["tok_to_cp"]) if tid >= 2}
    V = meta["vocab_size"]
    conf_toks = np.zeros((V, 8), dtype=np.int64)

    for tid in range(2, V):
        ch = chr(meta["tok_to_cp"][tid])
        code = word_to_code.get(ch)
        cands = []
        if code:
            # Same-code homographs + 1-key neighbor colliding characters
            for w, _ in code_to_words.get(code, []):
                if len(w) == 1 and w != ch and ord(w) in cp_to_tok:
                    cands.append(cp_to_tok[ord(w)])
            for pos in range(len(code)):
                for nb in neighbors.get(code[pos], []):
                    mc = code[:pos] + nb + code[pos + 1 :]
                    for w, _ in code_to_words.get(mc, [])[:2]:
                        if len(w) == 1 and w != ch and ord(w) in cp_to_tok:
                            cands.append(cp_to_tok[ord(w)])
        # Deduplicate & pad with random frequent tokens
        uniq = []
        for x in cands:
            if x not in uniq:
                uniq.append(x)
        while len(uniq) < 8:
            uniq.append(((tid * 131 + len(uniq) * 97) % (V - 2)) + 2)
        conf_toks[tid] = uniq[:8]
    return conf_toks


def export_model_wblm(model: MicroRwkv7Model, lut: np.ndarray, out_path: Path, parity_json_path: Path):
    model_cpu = model.cpu().eval()

    def q_rows(t: torch.Tensor):
        w = t.detach().float().numpy()
        max_abs = np.maximum(np.max(np.abs(w), axis=1), 1e-7).astype(np.float32)
        sc = (max_abs / 127.0).astype(np.float32)
        q = np.clip(np.round(w / sc[:, None]), -127, 127).astype(np.int8)
        return q, sc

    buf = bytearray()
    buf.extend(WBLM_MAGIC)
    buf.extend(struct.pack(
        "<6I",
        model_cpu.vocab_size, model_cpu.rank, model_cpu.d_model,
        model_cpu.n_heads, model_cpu.n_layers, model_cpu.ffn_dim,
    ))
    while len(buf) < 64:
        buf.append(0)

    def put(b: bytes):
        while len(buf) % 4 != 0:
            buf.append(0)
        buf.extend(b)

    def put_qm(w_t: torch.Tensor, b_t: torch.Tensor | None):
        q, sc = q_rows(w_t)
        put(q.tobytes())
        put(sc.astype("<f4").tobytes())
        if b_t is not None:
            put(b_t.detach().float().numpy().astype("<f4").tobytes())
        return q, sc

    put(lut.astype("<u2").tobytes())
    emb_q, emb_sc = q_rows(model_cpu.emb)
    put(emb_q.tobytes())
    put(emb_sc.astype("<f4").tobytes())
    uni_b = model_cpu.uni_bias.detach().float().numpy().astype("<f4")
    put(uni_b.tobytes())
    w_up_q, w_up_sc = put_qm(model_cpu.w_up.weight, None)

    qlayers = []
    for blk in model_cpu.blocks:
        ln1 = blk.ln1.detach().float().numpy().astype("<f4")
        mu_tm = blk.mu_tm.detach().float().numpy().astype("<f4")
        put(ln1.tobytes())
        put(mu_tm.tobytes())
        tp_b = blk.tm_proj.bias.detach().float().numpy().astype("<f4")
        tp_q, tp_sc = put_qm(blk.tm_proj.weight, blk.tm_proj.bias)
        rho = blk.rho.detach().float().numpy().astype("<f4")
        put(rho.tobytes())
        to_q, to_sc = put_qm(blk.tm_out.weight, None)
        ln2 = blk.ln2.detach().float().numpy().astype("<f4")
        mu_cm = blk.mu_cm.detach().float().numpy().astype("<f4")
        put(ln2.tobytes())
        put(mu_cm.tobytes())
        cu_b = blk.cm_up.bias.detach().float().numpy().astype("<f4")
        cu_q, cu_sc = put_qm(blk.cm_up.weight, blk.cm_up.bias)
        cd_q, cd_sc = put_qm(blk.cm_dn.weight, None)
        qlayers.append((ln1, mu_tm, tp_q, tp_sc, tp_b, rho, to_q, to_sc, ln2, mu_cm, cu_q, cu_sc, cu_b, cd_q, cd_sc))

    ln_out = model_cpu.ln_out.detach().float().numpy().astype("<f4")
    put(ln_out.tobytes())
    wdn_b = model_cpu.w_dn.bias.detach().float().numpy().astype("<f4")
    wdn_q, wdn_sc = put_qm(model_cpu.w_dn.weight, model_cpu.w_dn.bias)
    wmh_b = model_cpu.w_mtp_h.bias.detach().float().numpy().astype("<f4")
    wmh_q, wmh_sc = put_qm(model_cpu.w_mtp_h.weight, model_cpu.w_mtp_h.bias)
    wmc_q, wmc_sc = put_qm(model_cpu.w_mtp_c1.weight, None)
    # WBLM0003 omits the scheme-specific w_key1 head altogether; if exporting WBLM0003 with ext_vocab,
    # append pack_wblm0002_ext_vocab_bytes(ext_base_id, ext_vocab) directly after w_mtp_c1 (4-byte aligned).

    out_path.write_bytes(bytes(buf))
    print(f"[export] Wrote {out_path} ({len(buf)/(1024*1024):.2f} MB)", flush=True)

    # Compute exact quantized reference score on "今天天气真" -> "好" vs "油" for local Rust parity assertion!
    def qact(x):
        m = max(float(np.max(np.abs(x))), 1e-7)
        s = np.float32(m / 127.0)
        return np.clip(np.round(x / s), -127, 127).astype(np.int8), s

    def qmv(wq, wsc, b, xq, xsc):
        o = (wq.astype(np.int32) @ xq.astype(np.int32)).astype(np.float32) * wsc * xsc
        return o + b if b is not None else o

    def rnorm(x, g):
        return (x / np.sqrt(np.mean(x.astype(np.float32) ** 2) + 1e-5)) * g

    D, H = model_cpu.d_model, model_cpu.n_heads
    S = [np.zeros((H, 32, 32), dtype=np.float32) for _ in range(model_cpu.n_layers)]
    ptm = [np.zeros(D, dtype=np.float32) for _ in range(model_cpu.n_layers)]
    pcm = [np.zeros(D, dtype=np.float32) for _ in range(model_cpu.n_layers)]

    ctx_str = "今天天气真"
    toks = [0] + [int(lut[ord(c) - CJK_LUT_BASE]) for c in ctx_str]
    for tk in toks:
        x = qmv(w_up_q, w_up_sc, None, emb_q[tk], emb_sc[tk])
        for li, (ln1, mu_tm, tp_q, tp_sc, tp_b, rho, to_q, to_sc, ln2, mu_cm, cu_q, cu_sc, cu_b, cd_q, cd_sc) in enumerate(qlayers):
            u = rnorm(x, ln1)
            ssh = u + mu_tm * (ptm[li] - u)
            ptm[li] = u.copy()
            xq, xsc = qact(ssh)
            p6 = qmv(tp_q, tp_sc, tp_b, xq, xsc)
            ytm = np.zeros(D, dtype=np.float32)
            for h in range(H):
                b0 = h * 32
                rv, kr, vv = p6[b0:b0+32], p6[D+b0:D+b0+32], p6[2*D+b0:2*D+b0+32]
                wd = 1.0 / (1.0 + np.exp(-np.clip(p6[3*D+b0:3*D+b0+32], -20, 20)))
                bt = 1.0 / (1.0 + np.exp(-np.clip(p6[4*D+b0:4*D+b0+32], -20, 20)))
                gv = p6[5*D+b0:5*D+b0+32]
                sg = gv / (1.0 + np.exp(-np.clip(gv, -20, 20)))
                kh = kr / np.sqrt(np.sum(kr ** 2) + 1e-8)
                Sh = S[li][h] * wd[None, :]
                vp = Sh @ kh
                Sh = Sh + (bt * (vv - vp))[:, None] * kh[None, :]
                S[li][h] = Sh
                ytm[b0:b0+32] = (Sh @ rv + rho[b0:b0+32] * rv * vv) * sg
            yq, ysc = qact(ytm)
            x = x + qmv(to_q, to_sc, None, yq, ysc)
            u2 = rnorm(x, ln2)
            scm = u2 + mu_cm * (pcm[li] - u2)
            pcm[li] = u2.copy()
            cq, csc = qact(scm)
            ffn = np.maximum(qmv(cu_q, cu_sc, cu_b, cq, csc), 0.0) ** 2
            fq, fsc = qact(ffn)
            x = x + qmv(cd_q, cd_sc, None, fq, fsc)

    uo = rnorm(x, ln_out)
    hq, hsc = qact(uo)
    p1 = qmv(wdn_q, wdn_sc, wdn_b, hq, hsc)
    p1q, p1sc = qact(p1)

    def sc_char(c):
        tid = int(lut[ord(c) - CJK_LUT_BASE])
        return float((emb_q[tid].astype(np.int32) @ p1q.astype(np.int32)) * emb_sc[tid] * p1sc + uni_b[tid])

    parity = {
        "context": ctx_str,
        "score_hao": sc_char("好"),
        "score_re": sc_char("热"),
        "score_you": sc_char("油"),
        "score_tun": sc_char("突"),
    }
    parity_json_path.write_text(json.dumps(parity, indent=2, ensure_ascii=False))
    print(f"[parity sample] '{ctx_str}' -> 好:{parity['score_hao']:.3f}, 热:{parity['score_re']:.3f} vs 油:{parity['score_you']:.3f}, 突:{parity['score_tun']:.3f}", flush=True)


def run_training(work_dir: Path, seed_dir: Path, smoke_test: bool = False):
    out_dir = work_dir / "artifacts"
    out_dir.mkdir(parents=True, exist_ok=True)
    char_first_key, code_to_words, word_to_code = parse_wubi86_dict(seed_dir / "wubi86.dict.yaml")

    if smoke_test:
        clean_dir = work_dir / "clean_smoke"
        clean_dir.mkdir(parents=True, exist_ok=True)
        for g in ["formal", "community", "social"]:
            (clean_dir / f"{g}.txt").write_text("今天天气真好我们一起去吃饭吧\n输入法误触纠正效果非常不错\n" * 200, encoding="utf-8")
        meta, tok_bin = build_vocab_and_token_bin(clean_dir, out_dir, char_first_key, vocab_size=256)
    else:
        download_and_prepare_corpus(work_dir, seed_dir)
        meta, tok_bin = build_vocab_and_token_bin(work_dir / "clean", out_dir, char_first_key, vocab_size=8192)

    lut = np.load(out_dir / "lut.npy")
    device = torch.device("cuda" if torch.cuda.is_available() else ("mps" if torch.backends.mps.is_available() else "cpu"))
    print(f"[train] Using device: {device} ({torch.cuda.get_device_name(0) if device.type == 'cuda' else device.type})", flush=True)
    if device.type == "cuda":
        torch.set_float32_matmul_precision("high")
        verify_cuda_kernel_grad()

    tokens_mmap = np.memmap(tok_bin, dtype=np.uint16, mode="r")
    total_toks = len(tokens_mmap)

    model = MicroRwkv7Model(
        vocab_size=meta["vocab_size"],
        rank=128 if not smoke_test else 32,
        d_model=192 if not smoke_test else 64,
        n_heads=6 if not smoke_test else 2,
        n_layers=3 if not smoke_test else 2,
        ffn_dim=512 if not smoke_test else 128,
        init_uni_logp=meta["uni_logp"],
    ).to(device)

    n_params = sum(p.numel() for p in model.parameters())
    print(f"[train] MicroRwkv7Model total parameters: {n_params:,}", flush=True)

    tok_first_key_t = torch.tensor(meta["tok_first_key"], dtype=torch.long, device=device)
    conf_table_t = torch.tensor(
        build_wubi_confusable_table(meta, code_to_words, word_to_code),
        dtype=torch.long,
        device=device,
    )

    B = 256 if (device.type == "cuda" and not smoke_test) else 8
    T = 256 if not smoke_test else 32
    seq_span = T + 2

    # Stage 1: Full pretraining (~1.35 epochs over ~350M tokens = ~7,200 steps @ 65,536 tok/step = 472M tokens!)
    stage1_steps = 3 if smoke_test else max(6000, min(8000, int((total_toks * 1.35) // (B * T))))
    # Stage 2: QAT (INT8 STE) + Wubi Spatial-Confusable Contrastive Fine-Tuning
    stage2_steps = 2 if smoke_test else 1000
    total_steps = stage1_steps + stage2_steps

    optimizer = torch.optim.AdamW(model.parameters(), lr=3.0e-3, betas=(0.9, 0.98), weight_decay=0.01)
    use_amp = (device.type == "cuda" and torch.cuda.is_bf16_supported())
    amp_dtype = torch.bfloat16 if use_amp else torch.float32

    t_start = time.time()
    rng = np.random.default_rng(20261119)
    max_start = max(1, total_toks - seq_span - 1)

    print(f"[train] Running {stage1_steps} pretrain steps + {stage2_steps} QAT+Wubi-Contrastive steps (B={B}, T={T}, amp={amp_dtype})...", flush=True)

    for step in range(1, total_steps + 1):
        in_qat = (step > stage1_steps)
        # Cosine LR schedule: 3e-3 -> 3e-4 in Stage 1, 3e-4 -> 3e-5 in Stage 2
        if not in_qat:
            warmup = min(1.0, step / 250.0)
            prog = step / max(stage1_steps, 1)
            lr = 3.0e-4 + 0.5 * (3.0e-3 - 3.0e-4) * (1.0 + math.cos(math.pi * prog))
            lr *= warmup
        else:
            s2_prog = (step - stage1_steps) / max(stage2_steps, 1)
            lr = 3.0e-5 + 0.5 * (3.0e-4 - 3.0e-5) * (1.0 + math.cos(math.pi * s2_prog))
        for pg in optimizer.param_groups:
            pg["lr"] = lr

        starts = rng.integers(0, max_start, size=B)
        batch_np = np.stack([tokens_mmap[s : s + seq_span] for s in starts], axis=0).astype(np.int64)
        batch = torch.from_numpy(batch_np).to(device, non_blocking=True)

        optimizer.zero_grad(set_to_none=True)
        with torch.autocast(device_type=device.type, dtype=amp_dtype, enabled=use_amp):
            loss, ce1, ce2, lse_abs, h, p1, e_table = model.forward_train(batch, tok_first_key_t, qat=in_qat)

            # In Stage 2, add direct Wubi spatial-neighbor discriminative margin / listwise cross-entropy!
            if in_qat:
                tgt1 = batch[:, 1:-1]
                valid = (tgt1 >= 2)
                if valid.any():
                    p1_v = p1[valid]
                    pos_t = tgt1[valid]
                    if p1_v.shape[0] > 4096:
                        sel = torch.randperm(p1_v.shape[0], device=device)[:4096]
                        p1_v, pos_t = p1_v[sel], pos_t[sel]
                    neg_t = conf_table_t[pos_t]  # [N, 8] Wubi neighbor confusables!
                    cand_t = torch.cat([pos_t.unsqueeze(1), neg_t], dim=1)  # [N, 9]
                    cand_emb = e_table[cand_t]  # [N, 9, R]
                    cand_b = model.uni_bias[cand_t]  # [N, 9]
                    cand_scores = torch.einsum("nr,nkr->nk", p1_v.float(), cand_emb.float()) + cand_b.float()
                    wubi_ce = F.cross_entropy(cand_scores, torch.zeros(cand_scores.shape[0], dtype=torch.long, device=device))
                    loss = loss + 0.45 * wubi_ce

        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        optimizer.step()

        if step == 1 or step % 250 == 0 or step == stage1_steps or step == total_steps:
            elapsed = time.time() - t_start
            toks_done = step * B * T
            tps = toks_done / max(elapsed, 1e-3)
            phase = "QAT+Wubi" if in_qat else "Pretrain"
            print(
                f"  [{phase} {step:4d}/{total_steps}] lr={lr:.2e} | ce1={ce1.item():.3f} (ppl={math.exp(min(ce1.item(),15)):.1f}) "
                f"ce2={ce2.item():.3f} |logZ|={lse_abs.item():.3f} | {tps/1e3:.1f}k tok/s ({elapsed:.0f}s)",
                flush=True,
            )

    wblm_path = out_dir / "neural_rwkv7.wblm"
    parity_path = out_dir / "parity_check.json"
    export_model_wblm(model, lut, wblm_path, parity_path)

    # Tar-gz all needed artifacts into a single ~3MB archive for instant download
    bundle_path = work_dir / "rwkv7_artifacts.tar.gz"
    with tarfile.open(bundle_path, "w:gz") as tf:
        for fn in ["neural_rwkv7.wblm", "parity_check.json", "unigrams.tsv", "bigrams.tsv", "trigrams.tsv"]:
            fp = out_dir / fn
            if fp.exists():
                tf.add(fp, arcname=fn)
        if (work_dir / "clean" / "DONE.json").exists():
            tf.add(work_dir / "clean" / "DONE.json", arcname="corpus_stats_rwkv.json")
    print(f"[ALL DONE] Bundle ready: {bundle_path} ({bundle_path.stat().st_size/(1024*1024):.2f} MB)", flush=True)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Train Formordo Micro-RWKV-7 (WBLM0001) & N-gram tables")
    parser.add_argument("--work-dir", type=Path, default=None, help="Working directory for corpus & model outputs")
    parser.add_argument("--seed-dir", type=Path, default=None, help="Directory containing wubi86.dict.yaml & eval TSVs")
    parser.add_argument("--smoke", action="store_true", help="Run fast tiny smoke test")
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parent.parent
    base = args.work_dir or (Path("/tmp/rwkv_smoke") if args.smoke else Path("/content/rwkv_work"))
    seed = args.seed_dir or (repo_root / "Resources" / "RawTables" / "wubi86")
    base.mkdir(parents=True, exist_ok=True)
    run_training(base, seed, smoke_test=args.smoke)
