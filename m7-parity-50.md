# qlive parity

Historical sample before the embed/nested-cell skip fix. Embedded child documents
were not excluded, so this is not a verified no-execution run.

Corpus: `C:\dev\quarto-tests\tests\docs\smoke-all`. 50 documents run in 107 s.

| bucket | documents | meaning |
|---|---:|---|
| pass | 37 | warm output equals Quarto's |
| fallback | 0 | self-check turns warm off: slower, never wrong |
| **fail** | **9** | **warm output differs: a wrong preview** |
| error | 4 | Quarto could not render it as HTML |

Skipped from direct corpus renders (embeds were not filtered in this run):

- code cells (third-party code, not executed): 365
- declares an engine: 8
- inside a Quarto project: 151
- local extensions (third-party Lua, not executed): 22
- local filters or shortcodes (third-party Lua, not executed): 39

## Failures by cause

| cause | documents |
|---|---:|
| appendix: citations, footnotes, references (postprocessor) | 4 |
| heading/figure/table anchors (postprocessor) | 2 |
| other | 2 |
| table markup (postprocessor) | 1 |

## Failures

- `2022/09/29/ast-roundtrip-test.qmd` [table markup (postprocessor)]: Quarto: `…top table"><colgroup><col style="width: 62%"><col style="width: 15%"><col style="width: 19%"></colgr…` qlive: `…top table"><colgroup><col style="width: 56%"><col style="width: 13%"><col style="width: 17%"></colgr…`
- `2022/09/30/caption-footnotes/test.qmd` [other]: Quarto: `…sp;1: A schematic diagram of known types<span class="citation" data-cites="Harel79"><a class="footno…` qlive: `…sp;1: A schematic diagram of known types <span class="citation" data-cites="Harel79">[@Harel79]</spa…`
- `2023/01/02/citation-true.qmd` [appendix: citations, footnotes, references (postprocessor)]: Quarto: `…here</h2><p>This is a test</p></section><div class="default" id="quarto-appendix"><section class="qu…` qlive: `…here</h2><p>This is a test</p></section>…`
- `2023/02/01/4174.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset nav-…` qlive: `…<div class="panel-tabset nav-fill"><ul class="nav nav-tabs" role="tablis…`
- `2023/02/22/cite-as-only.qmd` [appendix: citations, footnotes, references (postprocessor)]: Quarto: `…this great interpretation.</p></section><div class="default" id="quarto-appendix"><section class="qu…` qlive: `…this great interpretation.</p></section>…`
- `2023/03/01/issue-4586.qmd` [appendix: citations, footnotes, references (postprocessor)]: Quarto: `…<div class="default" id="quarto-appendix"><section class="qu…` qlive: `……`
- `2023/03/03/article-layout/table-endnotes-4324.qmd` [heading/figure/table anchors (postprocessor)]: Quarto: `…rto-figure quarto-figure-center anchored page-columns page-full" id="tbl-fruit"><figure class="quart…` qlive: `…rto-figure quarto-figure-center anchored" id="tbl-fruit"><figure class="quarto-float quarto-float-tb…`
- `2023/03/03/article-layout/tabs-callouts-3280.qmd` [heading/figure/table anchors (postprocessor)]: Quarto: `…<section class="level2 page-columns page-full" id="header-1"><h2 class="anchored" …` qlive: `…<section class="level2" id="header-1"><h2 class="anchored" data-anchor-id="header-…`
- `2023/03/07/callout-footnote.qmd` [appendix: citations, footnotes, references (postprocessor)]: Quarto: `…teref"><sup>2</sup></a></p></div></div><div class="default" id="quarto-appendix"><section class="foo…` qlive: `…teref"><sup>2</sup></a></p></div></div><section class="footnotes footnotes-end-of-document" id="foot…`

## Errors

- `2022/10/31/include-notebook/embed-sub.qmd`: quarto render failed during capture: pandoc to: html output-file: embed-sub.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Included Resource T…
- `2022/10/31/include-notebook/embed.qmd`: quarto render failed during capture: pandoc to: html output-file: embed.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Included Resource Test …
- `2023/01/26/4068.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/03/03/issue-4621.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
