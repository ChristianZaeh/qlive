# qlive parity

Corpus: `C:\dev\quarto-tests\tests\docs\smoke-all`. 451 documents run in 1102 s.

| bucket | documents | meaning |
|---|---:|---|
| pass | 429 | warm output equals Quarto's |
| fallback | 0 | self-check turns warm off: slower, never wrong |
| **fail** | **12** | **warm output differs: a wrong preview** |
| error | 10 | Quarto could not render it as HTML |

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
| other | 8 |
| figure markup (postprocessor) | 4 |

## Failures

- `2023/04/04/issue-5084.qmd` [figure markup (postprocessor)]: Quarto: `…e"><p><img class="img-fluid figure-img" role="img" src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUg…` qlive: `…e"><p><img class="img-fluid figure-img" src="temp.png" title="tooltip"></p><figcaption>alt text</fig…`
- `2024/01/05/8066.qmd` [figure markup (postprocessor)]: Quarto: `…ure-img" src="https://placeholder.co/400.png"></p><figcaption class="margin-caption">Test figure cap…` qlive: `…ure-img" src="https://placeholder.co/400"></p><figcaption class="margin-caption">Test figure caption…`
- `2024/01/05/issue-8000.qmd` [other]: Quarto: `…ure-img" src="https://placeholder.co/400.png"></p><figcaption>An <em>image</em> with border</figcapt…` qlive: `…ure-img" src="https://placeholder.co/400"></p><figcaption>An <em>image</em> with border</figcaption>…`
- `2024/01/11/8234.qmd` [other]: Quarto: `…ure-img" src="https://placeholder.co/400.png"></p><figcaption>Data science project model</figcaption…` qlive: `…ure-img" src="https://placeholder.co/400"></p><figcaption>Data science project model</figcaption></f…`
- `2024/03/25/9190.qmd` [figure markup (postprocessor)]: Quarto: `…img" src="https://placeholder.co/400/400.png"></p><figcaption>Caption</figcaption></figure></div>…` qlive: `…img" src="https://placeholder.co/400/400"></p><figcaption>Caption</figcaption></figure></div>…`
- `2024/05/09/9631.qmd` [figure markup (postprocessor)]: Quarto: `…img" src="https://placeholder.co/400/400.png"></p><figcaption>Caption</figcaption></figure></div><di…` qlive: `…img" src="https://placeholder.co/400/400"></p><figcaption>Caption</figcaption></figure></div><div cl…`
- `2025/05/18/issue12766-3.qmd` [other]: Quarto: `…n id="eq-ab"><span class="math display"> \alpha = \beta \tag{1}</span></span></p><p>This is <a class…` qlive: `…n id="eq-ab"><span class="math display">\[ \alpha = \beta \tag{1}\]</span></span></p><p>This is <a c…`
- `2025/05/18/issue12766-4.qmd` [other]: Quarto: `…n id="eq-ab"><span class="math display"> \alpha = \beta \tag{1}</span></span></p><p>This is <a class…` qlive: `…n id="eq-ab"><span class="math display">\[ \alpha = \beta \tag{1}\]</span></span></p><p>This is <a c…`
- `fenced-div-reader-fixes/test-1.qmd` [other]: Quarto: `…<div data-key="value" id="id"><p>Some content inside a malfor…` qlive: `…<p>::: {#id key= value} Some content inside a malformed Pando…`
- `issues/3473-toc-side-body/body.qmd` [other]: Quarto: `…<nav id="TOC" role="doc-toc"><h2 id="toc-title">Contents</h2>…` qlive: `…<section class="level1" id="section-1"><h1>Section 1</h1><p>L…`
- `issues/3473-toc-side-body/left-body.qmd` [other]: Quarto: `…<nav id="TOC-body" role="doc-toc"><h2 id="toc-title">Contents…` qlive: `…<section class="level1" id="section-1"><h1>Section 1</h1><p>L…`
- `issues/3473-toc-side-body/right-body.qmd` [other]: Quarto: `…<nav id="TOC-body" role="doc-toc"><h2 id="toc-title">Contents…` qlive: `…<section class="level1" id="section-1"><h1>Section 1</h1><p>L…`

## Errors

- `2023/05/03/5189.qmd`: Quarto's page has no <main>
- `2024/05/14/placeholder-test.qmd`: quarto render failed during capture: pandoc to: html output-file: placeholder-test.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en [31mERROR (C:\Prog…
- `2024/05/20/9722.qmd`: quarto render failed during capture: pandoc to: html output-file: 9722.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en [31mERROR (C:\Program Files\Qu…
- `2024/06/06/9903.qmd`: quarto render failed during capture: pandoc to: html output-file: 9903.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en htmllink: https://www.example.c…
- `2024/06/27/10165.qmd`: quarto render failed during capture: pandoc to: html output-file: 10165.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: test [31mERROR (C:\Pro…
- `brand/minimal-brand.qmd`: Quarto's page has no <main>
- `typst/layout/fraction-layout.qmd`: quarto render failed during capture: pandoc to: html output-file: fraction-layout.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Fraction layo…
- `typst/typst-i18n.qmd`: quarto render failed during capture: pandoc to: html output-file: typst-i18n.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: es title: Typst internationa…
- `typst/typst-subfig-badid.qmd`: quarto render failed during capture: pandoc to: html output-file: typst-subfig-badid.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Typst id-l…
- `typst/typst-subfig.qmd`: quarto render failed during capture: pandoc to: html output-file: typst-subfig.html standalone: true section-divs: true html-math-method: mathjax wrap: none default-image-extension: png variables: {} metadata document-css: false link-citations: true date-format: long lang: en title: Typst id-less su…
