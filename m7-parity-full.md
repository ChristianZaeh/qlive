# qlive parity

Historical M7 run, Quarto 1.8.27, corpus revision `3977c9d`. This run predates the
embed/nested-cell skip fix and `--no-execute` enforcement. Some embedded renders
attempted kernel startup; this report must not be described as a no-execution run.

Corpus: `C:\dev\quarto-tests\tests\docs\smoke-all`. 470 documents run in 1135 s.

| bucket | documents | meaning |
|---|---:|---|
| pass | 403 | warm output equals Quarto's |
| fallback | 1 | self-check turns warm off: slower, never wrong |
| **fail** | **39** | **warm output differs: a wrong preview** |
| error | 27 | Quarto could not render it as HTML |

Skipped from direct corpus renders (embeds were not filtered in this run):

- code cells (third-party code, not executed): 365
- declares an engine: 8
- inside a Quarto project: 151
- local extensions (third-party Lua, not executed): 22
- local filters or shortcodes (third-party Lua, not executed): 39

## Failures by cause

| cause | documents |
|---|---:|
| other | 29 |
| figure markup (postprocessor) | 5 |
| heading/figure/table anchors (postprocessor) | 3 |
| table markup (postprocessor) | 2 |

## Failures

- `2022/09/29/ast-roundtrip-test.qmd` [table markup (postprocessor)]: Quarto: `…top table"><colgroup><col style="width: 62%"><col style="width: 15%"><col style="width: 19%"></colgr…` qlive: `…top table"><colgroup><col style="width: 56%"><col style="width: 13%"><col style="width: 17%"></colgr…`
- `2022/09/30/caption-footnotes/test.qmd` [other]: Quarto: `…sp;1: A schematic diagram of known types<span class="citation" data-cites="Harel79"><a class="footno…` qlive: `…sp;1: A schematic diagram of known types <span class="citation" data-cites="Harel79">[@Harel79]</spa…`
- `2023/02/01/4174.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset nav-…` qlive: `…<div class="panel-tabset nav-fill"><ul class="nav nav-tabs" role="tablis…`
- `2023/03/03/article-layout/table-endnotes-4324.qmd` [heading/figure/table anchors (postprocessor)]: Quarto: `…rto-figure quarto-figure-center anchored page-columns page-full" id="tbl-fruit"><figure class="quart…` qlive: `…rto-figure quarto-figure-center anchored" id="tbl-fruit"><figure class="quarto-float quarto-float-tb…`
- `2023/03/03/article-layout/tabs-callouts-3280.qmd` [heading/figure/table anchors (postprocessor)]: Quarto: `…<section class="level2 page-columns page-full" id="header-1"><h2 class="anchored" …` qlive: `…<section class="level2" id="header-1"><h2 class="anchored" data-anchor-id="header-…`
- `2023/04/04/issue-5084.qmd` [figure markup (postprocessor)]: Quarto: `…e"><p><img class="img-fluid figure-img" role="img" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUg…` qlive: `…e"><p><img class="img-fluid figure-img" src="temp.png" title="tooltip"></p><figcaption>alt text</fig…`
- `2023/09/27/7041-d.qmd` [other]: Quarto: `…<div class="no-row-height column-margin column-container"><div class="qu…` qlive: `…<div class="column-margin quarto-float quarto-figure quarto-figure-cente…`
- `2023/09/27/7041-e.qmd` [other]: Quarto: `…<div class="no-row-height column-margin column-container"><div class="qu…` qlive: `…<div class="column-margin quarto-float quarto-figure quarto-figure-cente…`
- `2023/10/19/7294.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset"><ul…` qlive: `…<div class="panel-tabset"><ul class="nav nav-tabs" role="tablist"><li cl…`
- `2023/11/13/7534.qmd` [other]: Quarto: `…f"><sup>1</sup></a> Some more text.</p><div class="no-row-height column-margin column-container"><di…` qlive: `…f"><sup>1</sup></a> Some more text.</p><section class="footnotes footnotes-end-of-document" id="foot…`
- `2023/11/20/7632.qmd` [other]: Quarto: `…class="citation" data-cites="knuth1984">(<a href="#ref-knuth1984" role="doc-biblioref">Knuth 1984</a…` qlive: `…class="citation" data-cites="knuth1984">[@knuth1984]</span>.</p><div class="callout callout-style-de…`
- `2024/01/05/8066.qmd` [figure markup (postprocessor)]: Quarto: `…ass="quarto-figure quarto-figure-center page-columns page-full"><figure class="figure page-columns p…` qlive: `…ass="quarto-figure quarto-figure-center margin-caption"><figure class="figure"><p><img class="img-fl…`
- `2024/01/05/issue-8000.qmd` [other]: Quarto: `…ure-img" src="https://placeholder.co/400.png"></p><figcaption>An <em>image</em> with border</figcapt…` qlive: `…ure-img" src="https://placeholder.co/400"></p><figcaption>An <em>image</em> with border</figcaption>…`
- `2024/01/11/8234.qmd` [other]: Quarto: `…ure-img" src="https://placeholder.co/400.png"></p><figcaption>Data science project model</figcaption…` qlive: `…ure-img" src="https://placeholder.co/400"></p><figcaption>Data science project model</figcaption></f…`
- `2024/01/16/8289.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset"><ul…` qlive: `…<div class="panel-tabset"><ul class="nav nav-tabs" role="tablist"><li cl…`
- `2024/01/19/docusaurus/docusaurus-tabsets.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset"><ul…` qlive: `…<div class="panel-tabset"><ul class="nav nav-tabs" role="tablist"><li cl…`
- `2024/01/24/8362.qmd` [other]: Quarto: `…hor-id="references">References</h3><div class="references csl-bib-body hanging-indent" data-entry-sp…` qlive: `…hor-id="references">References</h3><div id="refs" role="list"></div></section></section>…`
- `2024/02/03/8580-b.qmd` [other]: Quarto: `…span class="citation" data-cites="base">R Core Team (<a href="#ref-base" role="doc-biblioref">2023</…` qlive: `…span class="citation" data-cites="base">@base</span></td></tr><tr class="even"><td style="text-align…`
- `2024/02/03/8586.qmd` [other]: Quarto: `…span class="citation" data-cites="base">R Core Team (<a href="#ref-base" role="doc-biblioref">2023</…` qlive: `…span class="citation" data-cites="base">@base</span></td></tr><tr class="even"><td style="text-align…`
- `2024/03/14/9076.qmd` [figure markup (postprocessor)]: Quarto: `…on></div><div class="quarto-layout-panel page-columns page-full" id="fig-gapminder"><figure class="q…` qlive: `…on></div><div class="quarto-layout-panel" id="fig-gapminder"><figure class="quarto-float quarto-floa…`
- `2024/03/25/9190.qmd` [figure markup (postprocessor)]: Quarto: `…img" src="https://placeholder.co/400/400.png"></p><figcaption>Caption</figcaption></figure></div>…` qlive: `…img" src="https://placeholder.co/400/400"></p><figcaption>Caption</figcaption></figure></div>…`
- `2024/04/10/9179.qmd` [table markup (postprocessor)]: Quarto: `…op table"><colgroup><col style="width: 11%"></colgroup><tbody><tr class="odd"><td><h1 id="ad">ad</h1…` qlive: `…op table"><colgroup><col style="width: 10%"></colgroup><tbody><tr class="odd"><td><h1 id="ad">ad</h1…`
- `2024/05/06/9582.qmd` [other]: Quarto: `…<div class="no-row-height column-margin column-container"><div class="qu…` qlive: `…<div class="column-margin quarto-float quarto-figure quarto-figure-cente…`
- `2024/05/06/9593.qmd` [other]: Quarto: `…<div class="page-columns page-full"><p> If you’d like to pl…` qlive: `…<p><span class="column-margin">This is a span that has the cl…`
- `2024/05/09/9631.qmd` [figure markup (postprocessor)]: Quarto: `…img" src="https://placeholder.co/400/400.png"></p><figcaption>Caption</figcaption></figure></div><di…` qlive: `…img" src="https://placeholder.co/400/400"></p><figcaption>Caption</figcaption></figure></div><div cl…`
- `2024/06/20/10018.qmd` [other]: Quarto: `…<p>This is a playground for Quarto.</p><div class="page-columns page-full"><p>I’m confused… </p>…` qlive: `…<p>This is a playground for Quarto.</p><p>I’m confused… <span class="column-margin margin-aside"…`
- `2025/01/08/7260.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset"><ul…` qlive: `…<div class="panel-tabset"><ul class="nav nav-tabs" role="tablist"><li cl…`
- `2025/05/13/issue-12727.qmd` [other]: Quarto: `…<div class="tabset-margin-container"></div><div class="panel-tabset" dat…` qlive: `…<div class="panel-tabset" data-group="language"><ul class="nav nav-tabs"…`
- `2025/05/18/issue12766-3.qmd` [other]: Quarto: `…n id="eq-ab"><span class="math display"> \alpha = \beta \tag{1}</span></span></p><p>This is <a class…` qlive: `…n id="eq-ab"><span class="math display">\[ \alpha = \beta \tag{1}\]</span></span></p><p>This is <a c…`
- `2025/05/18/issue12766-4.qmd` [other]: Quarto: `…n id="eq-ab"><span class="math display"> \alpha = \beta \tag{1}</span></span></p><p>This is <a class…` qlive: `…n id="eq-ab"><span class="math display">\[ \alpha = \beta \tag{1}\]</span></span></p><p>This is <a c…`
- `article-layout/margin-location/reference-location-margin.qmd` [heading/figure/table anchors (postprocessor)]: Quarto: `…<section class="level2 page-columns page-full" id="callout"><h2 class="anchored" d…` qlive: `…<section class="level2" id="callout"><h2 class="anchored" data-anchor-id="callout"…`
- `crossrefs/float/latex/latex-raw-table-env-3.qmd` [other]: Quarto: `…"#cb1-2" tabindex="-1"></a><span class="co">#| label: tbl-test</span></span>
<span id="cb1-3"><a ari…` qlive: `…"#cb1-2" tabindex="-1"></a><span class="in">#| label: tbl-test</span></span>
<span id="cb1-3"><a ari…`
- `crossrefs/float/latex/latex-raw-table-env-4.qmd` [other]: Quarto: `…"#cb1-2" tabindex="-1"></a><span class="co">#| label: tbl-test</span></span>
<span id="cb1-3"><a ari…` qlive: `…"#cb1-2" tabindex="-1"></a><span class="in">#| label: tbl-test</span></span>
<span id="cb1-3"><a ari…`
- `fenced-div-reader-fixes/test-1.qmd` [other]: Quarto: `…<div data-key="value" id="id"><p>Some content inside a malfor…` qlive: `…<p>::: {#id key= value} Some content inside a malformed Pando…`
- `issues/3473-toc-side-body/body.qmd` [other]: Quarto: `…<nav id="TOC" role="doc-toc"><h2 id="toc-title">Contents</h2>…` qlive: `…<section class="level1" id="section-1"><h1>Section 1</h1><p>L…`
- `issues/3473-toc-side-body/left-body.qmd` [other]: Quarto: `…<nav id="TOC-body" role="doc-toc"><h2 id="toc-title">Contents…` qlive: `…<section class="level1" id="section-1"><h1>Section 1</h1><p>L…`
- `issues/3473-toc-side-body/right-body.qmd` [other]: Quarto: `…<nav id="TOC-body" role="doc-toc"><h2 id="toc-title">Contents…` qlive: `…<section class="level1" id="section-1"><h1>Section 1</h1><p>L…`
- `typst/typst-citeproc.qmd` [other]: Quarto: `…s="citation" data-cites="Cronbach_1951">Cronbach (<a href="#ref-Cronbach_1952" role="doc-biblioref">…` qlive: `…s="citation" data-cites="Cronbach_1951">[@Cronbach_1951, @Cronbach_1952]</span></p><section class="l…`
- `typst/typst-no-citeproc.qmd` [other]: Quarto: `…s="citation" data-cites="Cronbach_1951">Cronbach (<a href="#ref-Cronbach_1951" role="doc-biblioref">…` qlive: `…s="citation" data-cites="Cronbach_1951">@Cronbach_1951</span></p><p>Hello <span class="citation" dat…`

## Errors

- `2022/10/31/include-notebook/embed-sub.qmd`: quarto render failed during capture: pandoc to: html output-file: embed-sub.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Included Resource T…
- `2022/10/31/include-notebook/embed.qmd`: quarto render failed during capture: pandoc to: html output-file: embed.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Included Resource Test …
- `2023/01/26/4068.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/03/03/issue-4621.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/05/03/5189.qmd`: Quarto's page has no <main>
- `2023/10/24/7334.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/10/27/7381.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/11/02/7262.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/11/08/7522.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/11/15/4370.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2023/12/08/7619.qmd`: pandoc lua worker: string expected, got nil while retrieving function argument filepath while retrieving arguments for function directory stack traceback: C:\Program Files\Quarto\share/pandoc/datadir/init.lua:185: in upvalue 'scriptDir' C:\Program Files\Quarto\share/pandoc/datadir/init.lua:324: in u…
- `2024/04/25/lightbox-no-caption.qmd`: pandoc lua worker: string expected, got nil while retrieving function argument filepath while retrieving arguments for function directory stack traceback: C:\Program Files\Quarto\share/pandoc/datadir/init.lua:185: in upvalue 'scriptDir' C:\Program Files\Quarto\share/pandoc/datadir/init.lua:324: in u…
- `2024/05/14/placeholder-test.qmd`: quarto render failed during capture: pandoc to: html output-file: placeholder-test.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en [31mERROR (C:\Prog…
- `2024/05/20/9722.qmd`: quarto render failed during capture: pandoc to: html output-file: 9722.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en [31mERROR (C:\Program Files\Qu…
- `2024/05/29/9713/main.qmd`: quarto render failed during capture: Rendering qmd embeds [1/1] plots.qmd ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering C:\dev\quarto-tests\t…
- `2024/06/06/9903.qmd`: quarto render failed during capture: pandoc to: html output-file: 9903.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en htmllink: https://www.example.c…
- `2024/06/27/10165.qmd`: quarto render failed during capture: pandoc to: html output-file: 10165.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: test [31mERROR (C:\Pro…
- `2024/09/30/10931.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `2025/06/11/issue-12853/quarto-test/index.qmd`: quarto render failed during capture: Rendering qmd embeds [1/1] somefolder/somefile.qmd Starting python3 kernel...[IPKernelApp] WARNING | Kernel is running over TCP without encryption. All communication (including code and outputs) is sent in plain text and is susceptible to eavesdropping. Use IPC t…
- `article-layout/grid.qmd`: quarto render failed during capture: ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering files
- `brand/minimal-brand.qmd`: Quarto's page has no <main>
- `embed/tables/parent.qmd`: quarto render failed during capture: Rendering qmd embeds [1/1] inset-table.qmd ERROR: Error executing 'Rscript': Failed to spawn 'Rscript': entity not found Unable to locate an installed version of R. Install R from https://cloud.r-project.org/ WARN: Error encountered when rendering C:\dev\quarto-t…
- `jats/basic.qmd`: quarto render failed during capture: pandoc to: html output-file: basic.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png toc: true variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Sample Docume…
- `typst/layout/fraction-layout.qmd`: quarto render failed during capture: pandoc to: html output-file: fraction-layout.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Fraction layo…
- `typst/typst-i18n.qmd`: quarto render failed during capture: pandoc to: html output-file: typst-i18n.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: es title: Typst internationa…
- `typst/typst-subfig-badid.qmd`: quarto render failed during capture: pandoc to: html output-file: typst-subfig-badid.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Typst id-l…
- `typst/typst-subfig.qmd`: quarto render failed during capture: pandoc to: html output-file: typst-subfig.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Typst id-less su…
