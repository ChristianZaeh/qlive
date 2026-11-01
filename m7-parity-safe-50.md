# qlive parity

Corpus: `C:\dev\quarto-tests\tests\docs\smoke-all`. 50 documents run in 112 s.

| bucket | documents | meaning |
|---|---:|---|
| pass | 44 | warm output equals Quarto's |
| fallback | 0 | self-check turns warm off: slower, never wrong |
| **fail** | **6** | **warm output differs: a wrong preview** |
| error | 0 | Quarto could not render it as HTML |

Skipped, never executed:

- code cells (third-party code, not executed): 384
- declares an engine: 5
- embedded documents (may execute code): 11
- inside a Quarto project: 149
- local extensions (third-party Lua, not executed): 17
- local filters or shortcodes (third-party Lua, not executed): 38

## Failures by cause

| cause | documents |
|---|---:|
| heading/figure/table anchors (postprocessor) | 2 |
| other | 2 |
| figure markup (postprocessor) | 1 |
| table markup (postprocessor) | 1 |

## Failures

- `2022/09/29/ast-roundtrip-test.qmd` [table markup (postprocessor)]: Quarto: `…top table"><colgroup><col style="width: 62%"><col style="width: 15%"><col style="width: 19%"></colgr…` qlive: `…top table"><colgroup><col style="width: 56%"><col style="width: 13%"><col style="width: 17%"></colgr…`
- `2022/09/30/caption-footnotes/test.qmd` [other]: Quarto: `…sp;1: A schematic diagram of known types<span class="citation" data-cites="Harel79"><a class="footno…` qlive: `…sp;1: A schematic diagram of known types <span class="citation" data-cites="Harel79">[@Harel79]</spa…`
- `2023/02/01/4174.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset nav-…` qlive: `…<div class="panel-tabset nav-fill"><ul class="nav nav-tabs" role="tablis…`
- `2023/03/03/article-layout/table-endnotes-4324.qmd` [heading/figure/table anchors (postprocessor)]: Quarto: `…rto-figure quarto-figure-center anchored page-columns page-full" id="tbl-fruit"><figure class="quart…` qlive: `…rto-figure quarto-figure-center anchored" id="tbl-fruit"><figure class="quarto-float quarto-float-tb…`
- `2023/03/03/article-layout/tabs-callouts-3280.qmd` [heading/figure/table anchors (postprocessor)]: Quarto: `…<section class="level2 page-columns page-full" id="header-1"><h2 class="anchored" …` qlive: `…<section class="level2" id="header-1"><h2 class="anchored" data-anchor-id="header-…`
- `2023/04/04/issue-5084.qmd` [figure markup (postprocessor)]: Quarto: `…e"><p><img class="img-fluid figure-img" role="img" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUg…` qlive: `…e"><p><img class="img-fluid figure-img" src="temp.png" title="tooltip"></p><figcaption>alt text</fig…`
