//! Curated language, extension, and basename aliases for the in-house bundle.

use std::collections::BTreeSet;

pub const LANGUAGE_ALIASES: &[(&str, &str)] = &[
    ("1c", "bsl"),
    ("actionscript-3", "actionscript"),
    ("adoc", "asciidoc-asciidoctor"),
    ("apache", "apache-conf"),
    ("apl", "apl"),
    ("asciidoc", "asciidoc-asciidoctor"),
    ("bzl", "starlark"),
    ("bazel", "starlark"),
    ("bat", "batch-file"),
    ("batch", "batch-file"),
    ("be", "berry"),
    ("bib", "bibtex"),
    ("bird", "bird2"),
    ("c++", "cpp"),
    ("cbl", "cobol"),
    ("cc", "cpp"),
    ("c#", "csharp"),
    ("clar", "clarity"),
    ("clj", "clojure"),
    ("cljs", "clojure"),
    ("cmd", "batch-file"),
    ("cob", "cobol"),
    ("coffee", "coffeescript"),
    ("cjs", "javascript"),
    ("cl", "common-lisp"),
    ("code-owner", "codeowners"),
    ("codeowner", "codeowners"),
    ("common-lisp", "common-lisp"),
    ("commonlisp", "common-lisp"),
    ("coq", "coq"),
    ("csv", "comma-separated-values"),
    ("cs", "csharp"),
    ("cu", "cuda"),
    ("cuh", "cuda"),
    ("cxx", "cpp"),
    ("cls", "apex"),
    ("dm", "dream-maker"),
    ("dme", "dream-maker"),
    ("dmm", "dream-maker"),
    ("docker", "dockerfile"),
    ("dockerfile", "docker"),
    ("dockerignore", "git-ignore"),
    ("edn", "clojure"),
    ("el", "emacs-lisp"),
    ("elisp", "emacs-lisp"),
    ("emacs-lisp", "emacs-lisp"),
    ("env", "dotenv"),
    ("ejs", "ejs"),
    ("erb", "html-rails"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("f77", "fortran-fixed-form"),
    ("f90", "fortran-modern"),
    ("f95", "fortran-modern"),
    ("fortran", "fortran-modern"),
    ("fortran-free-form", "fortran-modern"),
    ("fs", "f#"),
    ("fsharp", "f#"),
    ("fnl", "fennel"),
    ("feature", "gherkin"),
    ("ftl", "fluent"),
    ("gjs", "glimmer-js"),
    ("gdscript", "gdscript-godot-engine"),
    ("gts", "glimmer-ts"),
    ("git-rebase", "git-rebase-todo"),
    ("gql", "graphql"),
    ("graphqls", "graphql"),
    ("gradle", "groovy"),
    ("haml", "ruby-haml"),
    ("hcl", "terraform"),
    ("hjson", "json"),
    ("hlsl", "hlsl"),
    ("hs", "haskell"),
    ("hx", "haxe"),
    ("hy", "hy"),
    ("html-derivative", "html"),
    ("jinja", "jinja2"),
    ("jinja-html", "html-jinja2"),
    ("jl", "julia"),
    ("ipynb", "json"),
    ("kql", "kusto"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("json5", "json"),
    ("jsonc", "json"),
    ("jsonl", "json"),
    ("jisonlex", "jison"),
    ("automount", "systemd"),
    ("just", "just"),
    ("justfile", "just"),
    ("lean", "lean-4"),
    ("lhs", "literate-haskell"),
    ("liquid", "liquid"),
    ("ll", "llvm"),
    ("lsp", "common-lisp"),
    ("md", "markdown"),
    ("mdx", "mdx"),
    ("ml", "ocaml"),
    ("mli", "ocaml"),
    ("mjs", "javascript"),
    ("mlir", "mlir"),
    ("mips", "mipsasm"),
    ("mount", "systemd"),
    ("msg", "rosmsg"),
    ("nb", "wolfram"),
    ("ndjson", "json"),
    ("ignorefile", "git-ignore"),
    ("js", "javascript"),
    ("javascript-babel", "jsx"),
    ("jsx", "jsx"),
    ("objective-cpp", "objective-c++"),
    ("node", "javascript"),
    ("objc", "objective-c"),
    ("objc++", "objective-c++"),
    ("pb", "protocol-buffer"),
    ("pbt", "protocol-buffer-text"),
    ("pot", "po"),
    ("pro", "prolog"),
    ("prolog", "prolog"),
    ("plsql", "sql"),
    ("postgres", "sql"),
    ("postgresql", "sql"),
    ("postcss", "css"),
    ("ql", "codeql"),
    ("properties", "java-properties"),
    ("proto", "protocol-buffer"),
    ("protobuf", "protocol-buffer"),
    ("ps1", "powershell"),
    ("psm1", "powershell"),
    ("psd1", "powershell"),
    ("ps", "powershell"),
    ("pwsh", "powershell"),
    ("python3", "python"),
    ("py", "python"),
    ("rb", "ruby"),
    ("regex", "regular-expression"),
    ("regexp", "regular-expression"),
    ("risc-v", "riscv"),
    ("rest", "restructuredtext"),
    ("rst", "restructuredtext"),
    ("rs", "rust"),
    ("scad", "openscad"),
    ("s", "asm"),
    ("scm", "scheme"),
    ("scheme", "scheme"),
    ("makefile", "make"),
    ("shell", "bash"),
    ("shellscript", "bash"),
    ("shell-session", "shell-unix-generic"),
    ("shellsession", "shell-unix-generic"),
    ("sh", "bash"),
    ("shader", "shaderlab"),
    ("slim", "ruby-slim"),
    ("sol", "solidity"),
    ("spl", "splunk"),
    ("spir-v", "spirv"),
    ("spirv-asm", "spirv"),
    ("sv", "systemverilog"),
    ("service", "systemd"),
    ("socket", "systemd"),
    ("scope", "systemd"),
    ("slice", "systemd"),
    ("srv", "rosmsg"),
    ("swap", "systemd"),
    ("system-verilog", "systemverilog"),
    ("target", "systemd"),
    ("td", "tablegen"),
    ("tfstate", "json"),
    ("tf", "terraform"),
    ("tfvars", "terraform"),
    ("ts", "typescript"),
    ("trigger", "apex"),
    ("tres", "gdresource"),
    ("tscn", "gdresource"),
    ("tsv", "tab-separated-values"),
    ("ttl", "turtle"),
    ("timer", "systemd"),
    ("twig", "twig"),
    ("typescriptreact", "tsx"),
    ("vim", "viml"),
    ("vimscript", "viml"),
    ("vue", "vue-component"),
    ("vue-html", "vue-component"),
    ("vue-vine", "vue-component"),
    ("wast", "wasm"),
    ("wat", "wasm"),
    ("wy", "wenyan"),
    ("wl", "wolfram"),
    ("wls", "wolfram"),
    ("x86asm", "x86-64-assembly"),
    ("xslt", "xsl"),
    ("yml", "yaml"),
    ("zs", "zenscript"),
    ("zsh", "bash"),
];

pub const EXTENSION_ALIASES: &[(&str, &str)] = &[
    ("bazel", "starlark"),
    ("bzl", "starlark"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("cjs", "javascript"),
    ("cls", "apex"),
    ("cs", "csharp"),
    ("cu", "cuda"),
    ("cuh", "cuda"),
    ("cxx", "cpp"),
    ("fs", "f#"),
    ("go", "go"),
    ("h", "c"),
    ("h++", "cpp"),
    ("hh", "cpp"),
    ("hpp", "cpp"),
    ("hxx", "cpp"),
    ("java", "java"),
    ("js", "javascript"),
    ("jsx", "jsx"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("lua", "lua"),
    ("md", "markdown"),
    ("mjs", "javascript"),
    ("mts", "typescript"),
    ("nix", "nix"),
    ("php", "php"),
    ("ps1", "powershell"),
    ("psm1", "powershell"),
    ("psd1", "powershell"),
    ("py", "python"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("cts", "typescript"),
    ("scss", "scss"),
    ("sh", "bash"),
    ("sql", "sql"),
    ("sv", "systemverilog"),
    ("swift", "swift"),
    ("tf", "terraform"),
    ("tfvars", "terraform"),
    ("toml", "toml"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("v", "verilog"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
];

pub const BASENAME_ALIASES: &[(&str, &str)] = &[
    ("BUILD", "starlark"),
    ("BUILD.bazel", "starlark"),
    ("WORKSPACE", "starlark"),
    ("WORKSPACE.bazel", "starlark"),
    ("MODULE.bazel", "starlark"),
    ("CODEOWNERS", "codeowners"),
    ("Dockerfile", "dockerfile"),
    ("Dockerfile", "docker"),
    ("Justfile", "just"),
    ("qmldir", "qmldir"),
    ("Makefile", "make"),
    ("GNUmakefile", "make"),
    ("BSDmakefile", "make"),
    ("CMakeLists.txt", "cmake"),
    (".bazelrc", "starlark"),
    (".clang-format", "yaml"),
    (".clang-tidy", "yaml"),
    (".dockerignore", "git-ignore"),
];

pub fn aliases_for_language(language: &str) -> Vec<String> {
    LANGUAGE_ALIASES
        .iter()
        .filter_map(|(alias, target)| (*target == language).then_some(normalize(alias)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn extensions_for_language(language: &str) -> Vec<String> {
    let target_for_extension = |extension: &str| {
        EXTENSION_ALIASES
            .iter()
            .find_map(|(candidate, target)| (*candidate == extension).then_some(*target))
    };
    let mut extensions = BTreeSet::new();
    if target_for_extension(language).is_none_or(|target| target == language) {
        extensions.insert(normalize(language));
    }
    for (alias, target) in LANGUAGE_ALIASES {
        if *target == language
            && target_for_extension(alias).is_none_or(|target| target == language)
        {
            extensions.insert(normalize(alias));
        }
    }
    for (extension, target) in EXTENSION_ALIASES {
        if *target == language {
            extensions.insert(normalize(extension));
        }
    }
    extensions.into_iter().collect()
}

pub fn basenames_for_language(language: &str) -> Vec<String> {
    BASENAME_ALIASES
        .iter()
        .filter_map(|(basename, target)| (*target == language).then_some((*basename).to_owned()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub fn normalize_language_token(token: &str) -> String {
    normalize(token)
}

fn normalize(token: &str) -> String {
    token.trim().trim_start_matches('.').to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_extension_overrides_keep_v_for_verilog() {
        assert!(!extensions_for_language("v").contains(&"v".to_owned()));
        assert!(extensions_for_language("verilog").contains(&"v".to_owned()));
        assert!(extensions_for_language("c").contains(&"h".to_owned()));
    }

    #[test]
    fn curated_basenames_are_case_preserved() {
        assert!(basenames_for_language("starlark").contains(&"BUILD".to_owned()));
        assert!(basenames_for_language("dockerfile").contains(&"Dockerfile".to_owned()));
        assert!(basenames_for_language("make").contains(&"Makefile".to_owned()));
    }

    #[test]
    fn core_30_aliases_and_extensions() {
        assert!(aliases_for_language("bash").contains(&"shellscript".to_owned()));
        assert!(aliases_for_language("bash").contains(&"sh".to_owned()));
        assert!(aliases_for_language("dockerfile").contains(&"docker".to_owned()));
        assert!(aliases_for_language("rust").contains(&"rs".to_owned()));
        assert!(aliases_for_language("typescript").contains(&"ts".to_owned()));
        assert!(extensions_for_language("python").contains(&"py".to_owned()));
        assert!(extensions_for_language("ruby").contains(&"rb".to_owned()));
        assert!(extensions_for_language("kotlin").contains(&"kt".to_owned()));
        assert!(extensions_for_language("terraform").contains(&"tf".to_owned()));
        assert!(extensions_for_language("powershell").contains(&"ps1".to_owned()));
    }
}
