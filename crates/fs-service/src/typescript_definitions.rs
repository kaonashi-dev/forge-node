use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{
    definition_rank_with_context, read_file, resolve_inside, FsError, SearchMatch, SearchResults,
    MAX_DEFINITION_SIGNATURE_BYTES, MAX_DIRECTORY_PATH_BYTES, MAX_FILE_BYTES,
};

const MAX_PACKAGES: usize = 32;
const MAX_ENTRIES: usize = 8192;
const MAX_SCAN_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn append(
    root: &Path,
    relative: &str,
    symbol: &str,
    limit: usize,
    results: &mut SearchResults,
) {
    if !matches!(
        Path::new(relative).extension().and_then(|e| e.to_str()),
        Some("ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs")
    ) {
        return;
    }
    let Ok(contents) = read_file(root, relative) else {
        results.truncated = true;
        return;
    };
    if contents.binary || contents.too_large {
        results.truncated = true;
        return;
    }
    let (packages, truncated) = imported_packages(&contents.text);
    results.truncated |= truncated;
    if packages.is_empty() {
        return;
    }
    if results.matches.len() >= limit {
        results.truncated = true;
        return;
    }
    let binding = imported_binding(&contents.text, symbol);
    let Ok(source) = resolve_inside(root, relative) else {
        return;
    };
    let Some(parent) = source.parent() else {
        return;
    };
    let mut scanner = Scanner {
        root,
        symbol,
        limit,
        results,
        entries: 0,
        bytes_left: MAX_SCAN_BYTES,
        visited: HashSet::new(),
    };
    for package in packages {
        if binding.is_some_and(|(imported_package, _)| imported_package != package) {
            continue;
        }
        scanner.symbol = binding.map_or(symbol, |(_, exported)| exported);
        match package_root(root, parent, package) {
            Some(Ok(path)) => scanner.scan(path),
            Some(Err(_)) => scanner.results.truncated = true,
            None => {}
        }
    }
}

fn package_root(root: &Path, parent: &Path, package: &str) -> Option<Result<PathBuf, FsError>> {
    for directory in parent.ancestors().take_while(|p| p.starts_with(root)) {
        let relative = directory
            .strip_prefix(root)
            .ok()?
            .join("node_modules")
            .join(package);
        let candidate = root.join(&relative);
        // An installed but escaping package must not fall through to a different version.
        if candidate.symlink_metadata().is_ok() {
            return Some(resolve_inside(root, relative.to_str()?));
        }
    }
    None
}

struct Scanner<'a> {
    root: &'a Path,
    symbol: &'a str,
    limit: usize,
    results: &'a mut SearchResults,
    entries: usize,
    bytes_left: usize,
    visited: HashSet<PathBuf>,
}

impl Scanner<'_> {
    fn scan(&mut self, package: PathBuf) {
        let mut pending = vec![package];
        while let Some(directory) = pending.pop() {
            if self.entries >= MAX_ENTRIES
                || self.bytes_left == 0
                || self.results.matches.len() >= self.limit
            {
                self.results.truncated = true;
                return;
            }
            if !self.visited.insert(directory.clone()) {
                continue;
            }
            let Ok(entries) = fs::read_dir(directory) else {
                self.results.truncated = true;
                continue;
            };
            let mut paths = Vec::new();
            for entry in entries {
                if self.entries >= MAX_ENTRIES {
                    self.results.truncated = true;
                    break;
                }
                self.entries += 1;
                if let Ok(entry) = entry {
                    paths.push(entry.path());
                } else {
                    self.results.truncated = true;
                }
            }
            paths.sort();
            for path in paths {
                if self.results.matches.len() >= self.limit || self.bytes_left == 0 {
                    self.results.truncated = true;
                    return;
                }
                let Some(relative) = path.strip_prefix(self.root).ok().and_then(Path::to_str)
                else {
                    self.results.truncated = true;
                    continue;
                };
                if relative.len() > MAX_DIRECTORY_PATH_BYTES {
                    self.results.truncated = true;
                    continue;
                }
                let Ok(resolved) = resolve_inside(self.root, relative) else {
                    self.results.truncated = true;
                    continue;
                };
                if resolved.is_dir() {
                    if !matches!(
                        path.file_name().and_then(|n| n.to_str()),
                        Some("node_modules" | ".git")
                    ) {
                        pending.push(resolved);
                    }
                } else if [".d.ts", ".d.mts", ".d.cts"]
                    .iter()
                    .any(|suffix| relative.ends_with(suffix))
                    && self.visited.insert(resolved.clone())
                {
                    self.read_declarations(relative, &resolved);
                }
            }
        }
    }

    fn read_declarations(&mut self, relative: &str, path: &Path) {
        let cap = self.bytes_left.min(MAX_FILE_BYTES);
        let Ok(metadata) = fs::metadata(path) else {
            self.results.truncated = true;
            return;
        };
        if !metadata.is_file() {
            return;
        }
        if metadata.len() > cap as u64 {
            self.results.truncated = true;
            return;
        }
        let Ok(file) = File::open(path) else {
            self.results.truncated = true;
            return;
        };
        let mut bytes = Vec::new();
        if file.take(cap as u64 + 1).read_to_end(&mut bytes).is_err() {
            self.results.truncated = true;
            return;
        }
        self.bytes_left = self.bytes_left.saturating_sub(bytes.len());
        if bytes.len() > cap || bytes.contains(&0) {
            self.results.truncated = true;
            return;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            self.results.truncated = true;
            return;
        };
        let mut block_comment = false;
        let mut lines = text.lines().enumerate();
        while let Some((index, line)) = lines.next() {
            let code = uncomment_line(line, &mut block_comment);
            if !code.contains(self.symbol) {
                continue;
            }
            if code.len() > MAX_DEFINITION_SIGNATURE_BYTES {
                self.results.truncated = true;
                continue;
            }
            let following = lines.clone().map(|(_, line)| line);
            let Some((column, _)) = definition_rank_with_context(code, following, self.symbol)
            else {
                continue;
            };
            if self.results.matches.len() >= self.limit {
                self.results.truncated = true;
                return;
            }
            self.results.matches.push(SearchMatch {
                path: relative.to_string(),
                line: index as u32 + 1,
                column: column + line[..line.len() - code.len()].chars().count() as u32,
                text: super::context_line(line),
                before: Vec::new(),
                after: Vec::new(),
            });
        }
    }
}

fn uncomment_line<'a>(mut line: &'a str, block: &mut bool) -> &'a str {
    loop {
        if *block {
            let Some((_, rest)) = line.split_once("*/") else {
                return "";
            };
            *block = false;
            line = rest;
        } else if line.trim_start().starts_with("/*") {
            *block = true;
            line = line.trim_start().strip_prefix("/*").unwrap_or("");
        } else {
            return if line.trim_start().starts_with("//") {
                ""
            } else {
                line
            };
        }
    }
}

fn imported_packages(text: &str) -> (BTreeSet<&str>, bool) {
    let mut packages = BTreeSet::new();
    let mut tokens = Tokens(text).peekable();
    let mut truncated = false;
    while let Some(token) = tokens.next() {
        let specifier = match token {
            Token::Word("from") => match tokens.peek() {
                Some(Token::Literal(value)) => Some(*value),
                _ => None,
            },
            Token::Word("import" | "require") => match tokens.peek() {
                Some(Token::Literal(value)) => Some(*value),
                Some(Token::Punctuation('(')) => {
                    tokens.next();
                    match tokens.peek() {
                        Some(Token::Literal(value)) => Some(*value),
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        };
        let Some(package) = specifier.and_then(package_name) else {
            continue;
        };
        if packages.len() < MAX_PACKAGES || packages.contains(package) {
            packages.insert(package);
        } else {
            truncated = true;
        }
    }
    (packages, truncated)
}

fn package_name(specifier: &str) -> Option<&str> {
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    let valid = |part: &str| {
        part.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    };
    if let Some(scope) = first.strip_prefix('@') {
        let name = parts.next()?;
        (valid(scope) && valid(name)).then_some(&specifier[..first.len() + 1 + name.len()])
    } else {
        valid(first).then_some(first)
    }
}

fn imported_binding<'a>(text: &'a str, symbol: &str) -> Option<(&'a str, &'a str)> {
    let mut tokens = Tokens(text).peekable();
    while let Some(token) = tokens.next() {
        if !matches!(token, Token::Word("import"))
            || matches!(tokens.peek(), Some(Token::Punctuation('(')))
        {
            continue;
        }
        let mut named = false;
        let mut original = None;
        let mut alias = None;
        while let Some(token) = tokens.next() {
            match token {
                Token::Punctuation('{') => named = true,
                Token::Punctuation('}') => named = false,
                Token::Punctuation(',' | ';') => {
                    original = None;
                    if matches!(token, Token::Punctuation(';')) {
                        break;
                    }
                }
                Token::Word("from") if !named => {
                    if let (Some(exported), Some(Token::Literal(specifier))) =
                        (alias, tokens.next())
                    {
                        if let Some(package) = package_name(specifier) {
                            return Some((package, exported));
                        }
                    }
                    break;
                }
                Token::Word("as") if named => {
                    if matches!(tokens.next(), Some(Token::Word(local)) if local == symbol) {
                        alias = original;
                    }
                }
                Token::Word(word) if named => {
                    original = Some(word);
                    if word == symbol && !matches!(tokens.peek(), Some(Token::Word("as"))) {
                        alias = Some(word);
                    }
                }
                Token::Literal(_) | Token::Punctuation('(') if !named => break,
                _ => {}
            }
        }
    }
    None
}

enum Token<'a> {
    Word(&'a str),
    Literal(&'a str),
    Punctuation(char),
}

struct Tokens<'a>(&'a str);

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.0 = self.0.trim_start();
            if self.0.starts_with("//") {
                self.0 = self.0.split_once('\n').map_or("", |(_, rest)| rest);
            } else if self.0.starts_with("/*") {
                self.0 = self.0.split_once("*/").map_or("", |(_, rest)| rest);
            } else {
                break;
            }
        }
        let first = self.0.chars().next()?;
        if matches!(first, '\'' | '"' | '`') {
            let rest = &self.0[first.len_utf8()..];
            let mut escaped = false;
            for (index, character) in rest.char_indices() {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == first {
                    self.0 = &rest[index + character.len_utf8()..];
                    return Some(if first == '`' {
                        Token::Punctuation('`')
                    } else {
                        Token::Literal(&rest[..index])
                    });
                }
            }
            self.0 = "";
            return None;
        }
        if first.is_alphanumeric() || matches!(first, '_' | '$') {
            let end = self
                .0
                .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '$')))
                .unwrap_or(self.0.len());
            let word = &self.0[..end];
            self.0 = &self.0[end..];
            Some(Token::Word(word))
        } else {
            self.0 = &self.0[first.len_utf8()..];
            Some(Token::Punctuation(first))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search_definitions, search_files, SearchKind};

    fn fixture() -> tempfile::TempDir {
        let tmp = super::super::tests::git_repo();
        fs::create_dir_all(tmp.path().join("src/payins/services")).unwrap();
        fs::create_dir_all(tmp.path().join("node_modules/bullmq/dist/esm/classes")).unwrap();
        fs::write(tmp.path().join(".gitignore"), "node_modules/\n").unwrap();
        for (package, text) in [
            ("@nestjs/common", "export declare function Injectable(options?: InjectableOptions): ClassDecorator;\nexport interface OnModuleInit {\n  onModuleInit(): unknown;\n}\n"),
            ("@nestjs/bullmq", "export declare function InjectQueue(name?: string): ParameterDecorator;\n"),
        ] {
            let directory = tmp.path().join("node_modules").join(package);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("index.d.ts"), text).unwrap();
        }
        fs::write(
            tmp.path().join("src/settlement-date.ts"),
            "export const SETTLEMENT_TIMEZONE = 'UTC';\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("src/payins/services/settlement-scheduler.service.ts"),
            "import { Injectable, OnModuleInit } from '@nestjs/common';\nimport { InjectQueue } from '@nestjs/bullmq';\nimport { Queue } from 'bullmq';\nimport { SETTLEMENT_TIMEZONE } from '@/provider-balances/settlement-date';\n@Injectable()\nexport class SettlementSchedulerService implements OnModuleInit {\n  constructor(@InjectQueue('payins') private readonly queue: Queue) {}\n  async onModuleInit() {\n    await this.queue.upsertJobScheduler('hourly-due-settlement', {}, {});\n  }\n}\n",
        )
        .unwrap();
        // BullMQ 5.58.5 splits the optional job template across these five lines.
        fs::write(
            tmp.path().join("node_modules/bullmq/dist/esm/classes/queue.d.ts"),
            "export declare class Queue<DataType, ResultType, NameType extends string> {\n    upsertJobScheduler(jobSchedulerId: NameType, repeatOpts: Omit<RepeatOptions, 'key'>, jobTemplate?: {\n        name?: NameType;\n        data?: DataType;\n        opts?: JobSchedulerTemplateOptions;\n    }): Promise<Job<DataType, ResultType, NameType>>;\n}\n/**\n * class Queue is documented here, not declared here.\n */\n",
        )
        .unwrap();
        tmp
    }

    const SOURCE: &str = "src/payins/services/settlement-scheduler.service.ts";
    const TYPES: &str = "node_modules/bullmq/dist/esm/classes/queue.d.ts";

    #[test]
    fn nestjs_bullmq_navigation_includes_ignored_multiline_dependency_types() {
        let tmp = fixture();
        let local =
            search_files(tmp.path(), "upsertJobScheduler", SearchKind::Definition, 64).unwrap();
        assert!(local.matches.is_empty());
        let found = search_definitions(tmp.path(), SOURCE, "upsertJobScheduler", 64).unwrap();
        assert!(!found.truncated);
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].path, TYPES);
        assert_eq!(found.matches[0].line, 2);
        assert_eq!(found.matches[0].column, 5);
        assert!(
            !read_file(tmp.path(), &found.matches[0].path)
                .unwrap()
                .too_large
        );

        for (symbol, path, line) in [
            ("Queue", TYPES, 1),
            ("SettlementSchedulerService", SOURCE, 6),
            ("Injectable", "node_modules/@nestjs/common/index.d.ts", 1),
            ("OnModuleInit", "node_modules/@nestjs/common/index.d.ts", 2),
            ("InjectQueue", "node_modules/@nestjs/bullmq/index.d.ts", 1),
            ("SETTLEMENT_TIMEZONE", "src/settlement-date.ts", 1),
        ] {
            let found = search_definitions(tmp.path(), SOURCE, symbol, 64).unwrap();
            assert_eq!(found.matches.len(), 1, "{symbol}");
            assert_eq!(found.matches[0].path, path);
            assert_eq!(found.matches[0].line, line);
        }
        let hook = search_definitions(tmp.path(), SOURCE, "onModuleInit", 64).unwrap();
        assert_eq!(hook.matches.len(), 2);
        assert_eq!(
            (hook.matches[0].path.as_str(), hook.matches[0].line),
            (SOURCE, 8)
        );
        assert_eq!(
            (hook.matches[1].path.as_str(), hook.matches[1].line),
            ("node_modules/@nestjs/common/index.d.ts", 3)
        );
        let tree = crate::list_files(tmp.path()).unwrap();
        assert!(tree
            .entries
            .iter()
            .all(|entry| !entry.path.starts_with("node_modules/")));
    }

    #[test]
    fn imported_packages_handle_scopes_subpaths_comments_and_multiline_imports() {
        let (packages, truncated) = imported_packages(
            "import {\n Queue as Jobs\n} from 'bullmq';\nimport type { Injectable } from '@nestjs/common/decorators';\nexport { InjectQueue } from '@nestjs/bullmq';\nconst x = require('other/types');\nconst y = import('dynamic');\nimport 'side-effect';\n// import 'comment';\n/* from 'block'; */\nconst fake = \"from 'string'\";\nconst template = `import 'template'`;\nimport { local } from './local';\nimport { alias } from '@/alias';\nimport 'node:fs';\n",
        );
        assert_eq!(
            packages.into_iter().collect::<Vec<_>>(),
            [
                "@nestjs/bullmq",
                "@nestjs/common",
                "bullmq",
                "dynamic",
                "other",
                "side-effect"
            ]
        );
        assert!(!truncated);
    }

    #[test]
    fn named_import_aliases_search_the_exported_declaration() {
        let tmp = fixture();
        fs::write(
            tmp.path().join(SOURCE),
            "import type { Queue as Jobs } from 'bullmq';\nconst queue: Jobs = makeQueue();\n",
        )
        .unwrap();
        let found = search_definitions(tmp.path(), SOURCE, "Jobs", 64).unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].path, TYPES);
        assert_eq!(found.matches[0].line, 1);
        assert!(!found.truncated);
        assert_eq!(imported_binding("// import { Queue as Jobs } from 'fake';\nimport { Other, Queue as Jobs, Third } from 'bullmq/subpath';", "Jobs"), Some(("bullmq", "Queue")));
        assert_eq!(
            imported_binding("import { Queue as Jobs } from './local';", "Jobs"),
            None
        );
        assert_eq!(
            imported_binding("import { Queue } from 'bullmq';", "Queue"),
            Some(("bullmq", "Queue"))
        );
        assert_eq!(
            imported_binding("import { Queue as Jobs } from 'bullmq';", "Queue"),
            None
        );
        assert_eq!(
            imported_binding("import { from as Source } from 'package';", "Source"),
            Some(("package", "from"))
        );
    }

    #[test]
    fn directly_imported_names_do_not_scan_other_packages() {
        let tmp = fixture();
        File::create(
            tmp.path()
                .join("node_modules/@nestjs/common/oversized.d.ts"),
        )
        .unwrap()
        .set_len(MAX_FILE_BYTES as u64 + 1)
        .unwrap();
        let found = search_definitions(tmp.path(), SOURCE, "Queue", 64).unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].path, TYPES);
        assert!(!found.truncated);
    }

    #[test]
    fn a_full_candidate_list_does_not_read_dependency_files() {
        let tmp = fixture();
        let mut found = SearchResults {
            matches: vec![super::super::tests::hit(
                SOURCE,
                8,
                "async onModuleInit() {}",
            )],
            truncated: false,
        };
        let mut scanner = Scanner {
            root: tmp.path(),
            symbol: "Queue",
            limit: 1,
            results: &mut found,
            entries: 0,
            bytes_left: MAX_SCAN_BYTES,
            visited: HashSet::new(),
        };
        scanner.scan(tmp.path().join("node_modules/bullmq"));
        assert_eq!(scanner.entries, 0);
        assert_eq!(scanner.bytes_left, MAX_SCAN_BYTES);
        assert!(scanner.results.truncated);
    }

    #[test]
    fn dependency_search_uses_only_imported_packages_and_nearest_installation() {
        let tmp = fixture();
        fs::create_dir_all(tmp.path().join("node_modules/unrelated")).unwrap();
        fs::write(
            tmp.path().join("node_modules/unrelated/index.d.ts"),
            "declare class Queue {}\n",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src/node_modules/bullmq")).unwrap();
        fs::write(
            tmp.path().join("src/node_modules/bullmq/index.d.ts"),
            "export declare class Queue {}\n",
        )
        .unwrap();
        let found = search_definitions(tmp.path(), SOURCE, "Queue", 64).unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].path, "src/node_modules/bullmq/index.d.ts");
    }

    #[test]
    fn internal_package_symlinks_work_but_external_files_are_not_candidates() {
        let tmp = fixture();
        let outside = tempfile::tempdir().unwrap();
        fs::write(
            outside.path().join("outside.d.ts"),
            "export declare class Queue {}\n",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.d.ts"),
            tmp.path().join("node_modules/bullmq/outside.d.ts"),
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("src/node_modules")).unwrap();
        std::os::unix::fs::symlink(
            "../../node_modules/bullmq",
            tmp.path().join("src/node_modules/bullmq"),
        )
        .unwrap();
        let found = search_definitions(tmp.path(), SOURCE, "Queue", 64).unwrap();
        assert_eq!(found.matches.len(), 1);
        assert_eq!(found.matches[0].path, TYPES);
        assert!(found.truncated);
        assert!(search_definitions(tmp.path(), "../outside.ts", "Queue", 64).is_err());
        fs::remove_file(tmp.path().join("src/node_modules/bullmq")).unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("src/node_modules/bullmq"))
            .unwrap();
        let escaping = search_definitions(tmp.path(), SOURCE, "Queue", 64).unwrap();
        assert!(escaping.matches.is_empty());
        assert!(escaping.truncated);
    }

    #[test]
    fn oversized_types_and_candidate_limits_report_partial_searches() {
        let tmp = fixture();
        File::create(tmp.path().join("node_modules/bullmq/oversized.d.ts"))
            .unwrap()
            .set_len(MAX_FILE_BYTES as u64 + 1)
            .unwrap();
        let found = search_definitions(tmp.path(), SOURCE, "upsertJobScheduler", 64).unwrap();
        assert_eq!(found.matches.len(), 1);
        assert!(found.truncated);
        fs::write(
            tmp.path().join("node_modules/bullmq/second.d.ts"),
            "export declare class Queue {}\n",
        )
        .unwrap();
        let found = search_definitions(tmp.path(), SOURCE, "Queue", 1).unwrap();
        assert_eq!(found.matches.len(), 1);
        assert!(found.truncated);
        let invalid = search_definitions(tmp.path(), SOURCE, ".*", 64).unwrap();
        assert!(invalid.matches.is_empty());
        assert!(!invalid.truncated);
    }
}
