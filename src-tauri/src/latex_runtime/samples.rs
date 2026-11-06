//! Documents compiled during preparation besides the built-in templates, so a
//! prepared environment also covers the common setups of users' own projects
//! offline: the standard classes at 10/11/12pt, beamer, a common package set
//! with BibTeX, and Chinese through ctex.
//!
//! Measured on 2026-10-31 (Tectonic 0.17.0, bundle v33): these add about
//! 24 MB to the template cache, 19 MB of it the Fandol fonts that a plain
//! `ctexart` selects under Tectonic. `fontset=windows` reads the system fonts
//! from `C:\Windows\Fonts` by name and adds almost nothing.

const PACKAGES: &str = r"\documentclass[11pt]{article}
\usepackage[T1]{fontenc}
\usepackage[margin=25mm]{geometry}
\usepackage{amsmath,amssymb}
\usepackage{graphicx}
\usepackage{xcolor}
\usepackage{booktabs}
\usepackage{tikz}
\usepackage[numbers]{natbib}
\usepackage{hyperref}
\begin{document}
\section{Math}
\begin{align} a &= b + c \label{eq:a} \\ \mathbb{R} &\subseteq \mathbb{C} \end{align}
See \eqref{eq:a} and \citep{knuth}. $\forall \epsilon > 0, \exists \delta$.
\section{Table}
\begin{tabular}{lr}\toprule Name & Value \\ \midrule A & 1 \\ \bottomrule\end{tabular}
\section{Figure}
\textcolor{blue}{Blue} \colorbox{yellow}{box}
\begin{tikzpicture}\draw[->] (0,0) -- (2,1) node[right]{$x$}; \fill[red] (1,0) circle (2pt);\end{tikzpicture}
\scalebox{2}{scaled}
\bibliographystyle{plainnat}
\bibliography{refs}
\end{document}
";

const REFS: &str = "@book{knuth, author = {Donald E. Knuth}, title = {The {\\TeX}book}, publisher = {Addison-Wesley}, year = {1984}}\n";

const BEAMER: &str = r"\documentclass{beamer}
\title{Sample}\author{OpenNote}\date{2026-01-01}
\begin{document}
\frame{\titlepage}
\begin{frame}{Frame}
\begin{itemize}\item<1-> One \item<2-> Two\end{itemize}
\begin{block}{Block}$E = mc^2$\end{block}
\end{frame}
\end{document}
";

fn standard_class(class: &str, size: u8) -> String {
    let sectioning = if class == "article" {
        r"\section{Introduction}"
    } else {
        r"\chapter{Introduction}\section{Background}"
    };
    format!(
        r"\documentclass[{size}pt]{{{class}}}
\title{{Sample}}\author{{OpenNote}}\date{{2026-01-01}}
\begin{{document}}
\maketitle
{sectioning}
Plain text with \textbf{{bold}}, \textit{{italic}}, \texttt{{mono}}, \textsc{{Small Caps}} and $x^2 + \sqrt{{y}}$ and
\[ \int_0^1 f(x)\,dx = \sum_{{n=1}}^{{\infty}} \frac{{1}}{{n^2}}. \]
\begin{{itemize}}\item One \item Two\end{{itemize}}
\footnote{{A footnote.}}
\end{{document}}
"
    )
}

fn ctex(options: &str) -> String {
    format!(
        r"\documentclass{options}{{ctexart}}
\begin{{document}}
\section{{中文测试}}
这是一段中文正文，包含\textbf{{粗体}}和\emph{{强调}}以及数学 $a^2+b^2=c^2$。
\end{{document}}
"
    )
}

/// `(name, files)` per sample, in the shape of the template list; each has
/// `main.tex`.
pub(crate) fn preparation_samples() -> Vec<(String, Vec<(String, String)>)> {
    let main = |text: String| vec![("main.tex".to_owned(), text)];
    let mut samples = Vec::new();
    for class in ["article", "report", "book"] {
        for size in [10, 11, 12] {
            samples.push((
                format!("sample-{class}-{size}pt"),
                main(standard_class(class, size)),
            ));
        }
    }
    samples.push(("sample-beamer".to_owned(), main(BEAMER.to_owned())));
    samples.push((
        "sample-packages".to_owned(),
        vec![
            ("main.tex".to_owned(), PACKAGES.to_owned()),
            ("refs.bib".to_owned(), REFS.to_owned()),
        ],
    ));
    samples.push(("sample-ctex".to_owned(), main(ctex(""))));
    samples.push((
        "sample-ctex-windows".to_owned(),
        main(ctex("[fontset=windows]")),
    ));
    samples
}
