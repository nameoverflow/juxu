# Third-Party Data & Licenses

句序 (Juxu) 的离线词典（`Resources/Dicts/*.wbdict`、`Resources/Dicts/pinyin.pydict` 与 `Resources/Dicts/wubi_aux.wbaux`）由 `Core/src/bin/build_dicts.rs` 从 `Resources/RawTables/` 中的以下第三方开源码表与拼音数据直接编译生成（可通过 `make dicts` 完全复现）：

| 资源目录 | 上游仓库 | 固定提交 (Commit) | 包含文件 | 对应编译产物 | 开源协议 (SPDX) | 协议原文 |
| --- | --- | --- | --- | --- | --- | --- |
| `Resources/RawTables/wubi86/` | [rime/rime-wubi](https://github.com/rime/rime-wubi) | `152a0d3f3efe40cae216d1e3b338242446848d07` | `wubi86.dict.yaml`, `AUTHORS`, `LICENSE` | `wubi86.wbdict`, `wubi_aux.wbaux` | `LGPL-3.0-only` | [wubi86/LICENSE](Resources/RawTables/wubi86/LICENSE) |
| `Resources/RawTables/wubi98/` | [lotem/rime-wubi98](https://github.com/lotem/rime-wubi98) | `0bc5b2b296f536d38ffe38381d37e5011da55e59` | `wubi98.dict.yaml`, `LICENSE` | `wubi98.wbdict`, `wubi_aux.wbaux` | `GPL-3.0-only` | [wubi98/LICENSE](Resources/RawTables/wubi98/LICENSE) |
| `Resources/RawTables/wubi06/` | [g178253/rime-wubi06](https://github.com/g178253/rime-wubi06) | `b10733bb001c0b38917577f51d78834bdc56a35f` | `wubi06.dict.yaml`, `LICENSE` | `wubi06.wbdict`, `wubi_aux.wbaux` | `Apache-2.0` | [wubi06/LICENSE](Resources/RawTables/wubi06/LICENSE) |
| `Resources/RawTables/pinyin/` | [mozillazg/pinyin-data](https://github.com/mozillazg/pinyin-data) (v0.16.0) | `fe29ad32bdf352b31a02b9936e537f575432e9a6` | `kTGHZ2013.txt`, `kMandarin_8105.txt`, `pinyin.txt`, `LICENSE` | `wubi_aux.wbaux` | `MIT` | [pinyin/LICENSE](Resources/RawTables/pinyin/LICENSE) |
| `Resources/RawTables/cangjie3/` | [cangjie-system/rime-cangjie-integrated](https://github.com/cangjie-system/rime-cangjie-integrated) | `master` | `cangjie.3-base.dict.yaml`, `cangjie.3-base-dups.dict.yaml`, `LICENSE` | `cangjie3.wbdict` | `LGPL-3.0-only` | [cangjie3/LICENSE](Resources/RawTables/cangjie3/LICENSE) |
| `Resources/RawTables/cangjie5/` | [rime/rime-cangjie](https://github.com/rime/rime-cangjie) | `master` | `cangjie5.base.dict.yaml`, `cangjie5.extended.dict.yaml`, `LICENSE` | `cangjie5.wbdict` | `LGPL-3.0-only` | [cangjie5/LICENSE](Resources/RawTables/cangjie5/LICENSE) |
| `Resources/RawTables/cangjie6/` | [rime-aca/rime-cangjie6](https://github.com/rime-aca/rime-cangjie6) | `master` | `cangjie6.dict.yaml`, `README.md` | `cangjie6.wbdict` | `GPL-3.0-only` | [cangjie6/README.md](Resources/RawTables/cangjie6/README.md) |

## 著作权与衍生来源说明

1. **`rime/rime-wubi`（五笔 86 版）**
   - 协议：GNU Lesser General Public License v3.0 (`LGPL-3.0`)。
   - 署名（摘自 `AUTHORS` 与 `wubi86.dict.yaml` 头部）：衍生自 Yu Yuwei (`acevery`) 维护的 `ibus-table`（LGPL）及陈兴、Gong Chen 等人的整理转换，基础字根与码表源自王永民先生（Public Domain）。
2. **`lotem/rime-wubi98`（五笔 98 版）**
   - 协议：GNU General Public License v3.0 (`GPL-3.0`)，声明于 `wubi98.dict.yaml` 文件头部 `# license: GPLv3`。
   - 署名：衍生自 `https://github.com/acevery/ibus-table-wubi`，由 Gong Chen (`lotem`) 整理为 Rime 词典格式。上游仓库未附独立文本文件，本仓库已在 `Resources/RawTables/wubi98/LICENSE` 补齐官方 GPL-3.0 协议全文。
3. **`g178253/rime-wubi06`（新世纪 / 06 版五笔）**
   - 协议：Apache License 2.0 (`Apache-2.0`)，协议原文见 `Resources/RawTables/wubi06/LICENSE`。
4. **`mozillazg/pinyin-data`（拼音反查与通用规范汉字表）**
   - 协议：MIT License (`MIT`)，Copyright (c) 2016 mozillazg，协议原文见 `Resources/RawTables/pinyin/LICENSE`。
   - 原始数据整合自 [Unicode Unihan Database](https://www.unicode.org/reports/tr38/)（`kTGHZ2013` / `kMandarin` 等字段）及《通用规范汉字表》（2013 年版）。

5. **`iDvel/rime-ice`（Emoji 候选词表）**
   - 协议：GNU General Public License v3.0 (`GPL-3.0`)，协议原文见 `Resources/RawTables/pinyin_ice/LICENSE`。
   - `Resources/Dicts/emoji.tsv` 取自上游 `opencc/emoji.txt`，仅按词条字节序重新排序。它作为独立资源文件分发，运行时只读映射，不编译进 `Core/` 静态库。

## 纠错与整句语言模型语料来源（`wubi_aux.wbaux` `WBAUX003` / `WBLM0003`）

### V4 中英混输无方案绑定语言模型训练语料（`WBLM0003`）

当前随附 `Resources/Dicts/wubi_aux.wbaux`（`WBAUX003`）中的 `Micro-RWKV-7` 量化神经语言模型权重（`WBLM0003`，无方案相关头，`INT8` 只读 `mmap`）及高 PMI N-gram 统计表，基于以下开源预训练与指令蒸馏语料离线统计与权重蒸馏生成（**仅提取聚合统计与量化模型参数，仓库与编译产物中零原始语料文本再分发**）：

| 语料名称 | 上游数据集 / 项目 | 用途 | 开源协议声明 | 分发与归属说明 |
| --- | --- | --- | --- | --- |
| `MAP-NEO` | [m-a-p/Matrix (MAP-Neo Open Pretraining & SFT Corpus)](https://huggingface.co/m-a-p) | 中英双语及代码切换预训练/指令权重蒸馏 | `Apache-2.0` / `ODC-BY` | 保留归属说明；零原始文本再分发 |
| `FineWeb-Edu-Chinese` | [opencsg/Fineweb-Edu-Chinese-V2.1](https://huggingface.co/datasets/opencsg/Fineweb-Edu-Chinese-V2.1) | 现代高质量中文网页与科普书面语权重蒸馏 | `ODC-BY` | 保留 OpenCSG 归属说明；零原始文本再分发 |
| `SmolTalk-Chinese` | [opencsg/smoltalk-chinese](https://huggingface.co/datasets/opencsg/smoltalk-chinese) | 现代自然中文多轮对话与日常表达权重蒸馏 | `Apache-2.0` | 保留归属说明；零原始文本再分发 |
| `Chinese-DeepSeek-R1-Distill` | [Congliu/Chinese-DeepSeek-R1-Distill-data-110k](https://huggingface.co/datasets/Congliu/Chinese-DeepSeek-R1-Distill-data-110k) | 现代技术/推理表达与中英术语衔接权重蒸馏 | `Apache-2.0` | 保留归属说明；零原始文本再分发 |

### 早期实验语料管线说明（`Scripts/train_rwkv7.py` / `Scripts/corpus_v4.py`）

仓库离线实验脚本 `Scripts/train_rwkv7.py` 与 `Scripts/corpus_v4.py` 另支持按需拉取多域公开语料从头复现 `WBLM0003` 权重与精简高 PMI N-gram 统计表（8,051 字 Unigram、80,000 条 Bigram、35,000 条 Trigram，同样不含任何原始语料原文），并通过 `build-dicts --corpus-lm` 编译生成：

| 语料分组 | 上游数据集 | 采样规模（去重汉字） | 上游协议声明 | 分发/商用合规提示 |
| --- | --- | --- | --- | --- |
| 日常社交对话 (Social) | [silver/lccc (`lccc_base_train`)](https://huggingface.co/datasets/silver/lccc) | 95.00M | `MIT` | 需保留署名声明 |
| 日常社交评论 (Social) | [theblackcat102/bilibili_comments_sharegpt](https://huggingface.co/datasets/theblackcat102/bilibili_comments_sharegpt) | ~2.18M | 公开社区语料 | 原始评论受平台条款约束 |
| 社交生活笔记 (Social) | [FreshCrawl/rednote-xiaohongshu-notes](https://huggingface.co/datasets/FreshCrawl/rednote-xiaohongshu-notes) | ~1.49M | `Research-only`（非商用研究） | **仅限非商用研究**，公开发布商业版本请勿启用该分片 |
| 现代社区问答 (Community) | [KrisTHL181/zhihu-qa](https://huggingface.co/datasets/KrisTHL181/zhihu-qa)（筛 2024.06–2026 分片 `86..93`） | 71.09M | `ODC-BY` | 需署名；原始抓取内容受平台条款约束 |
| 现代正式文本 (Formal) | [opencsg/Fineweb-Edu-Chinese-V2.1 (`4_5`)](https://huggingface.co/datasets/opencsg/Fineweb-Edu-Chinese-V2.1) | 75.00M | `ODC-BY` | 需保留归属说明 |
| 百科书面语 (Formal) | [wikimedia/wikipedia (`20231101.zh`)](https://huggingface.co/datasets/wikimedia/wikipedia) | 35.00M（繁转简并过滤自动生成小条目） | `CC-BY-SA-4.0` & `GFDL` | 需署名；统计/模型派生权重建议保留来源说明 |

> 若需构建不带神经语言模型的最小回退版 `wubi_aux.wbaux`，可在未放置预训练 LM 产物时运行 `make dicts`（不传 `JUXU_CORPUS_LM`），退回仅由 `Resources/RawTables/` 生成的基础 Bigram 统计。

## 派生二进制产物与协议兼容性提示

- `Resources/Dicts/*.wbdict` 分别是对应原始码表的直接二进制索引编译产物，沿用各自原始码表的开源协议（`LGPL-3.0` / `GPL-3.0` / `Apache-2.0`）。
- `Resources/Dicts/wubi_aux.wbaux`（`WBAUX003`）汇总了 `mozillazg/pinyin-data` 的拼音反查索引、五笔词表的回退统计，以及离线蒸馏的 `Micro-RWKV-7`（`WBLM0003` `INT8` 权重）与高 PMI N-gram 残差表（不含任何原始训练文本）。分发 `Resources/Dicts/` 时需同时遵守 `GPL-3.0` / `LGPL-3.0` / `Apache-2.0` / `MIT` / `ODC-BY` 条款；若自行使用 `Scripts/train_rwkv7.py` 重训商业分发版本，请确保排除 `Research-only` 社交语料分片。
