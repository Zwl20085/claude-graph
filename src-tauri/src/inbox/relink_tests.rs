use std::path::PathBuf;

use super::*;

const HOME: &str = r"C:\Users\Owner";
const ROUND: &str = r"C:\Users\Owner\AppData\Roaming\claude-graph\inbox\rounds\survey-0123abcd\20261110-090000-ab12";
const DATA: &str = r"C:\Users\Owner\AppData\Roaming\claude-graph";
const TIPS: &str = r"C:\Users\Owner\OneDrive\Vindictive";
const D: &str = r"C:\Users\Owner\AppData\Roaming\claude-graph\inbox\rounds\survey-0123abcd\20261110-090000-ab12\nodes\task\deliverables";

fn places() -> Places {
    let round = PathBuf::from(ROUND);
    Places {
        tip_id: "survey".into(),
        deliverables: vec![
            round.join("nodes").join("task").join("deliverables"),
            PathBuf::from(
                r"%APPDATA%\claude-graph\inbox\rounds\survey-0123abcd\20261110-090000-ab12\nodes\task\deliverables",
            ),
            // The 8.3 spelling of the same folder (as GetShortPathName gives).
            PathBuf::from(
                r"C:\Users\OWNER~1\AppData\Roaming\CLAUDE~1\inbox\rounds\SURVEY~1\20261~1\nodes\task\DELIVE~1",
            ),
        ],
        private: vec![
            round,
            PathBuf::from(DATA),
            PathBuf::from(r"C:\Users\OWNER~1\AppData\Roaming\CLAUDE~1"),
            PathBuf::from(r"%APPDATA%\claude-graph"),
        ],
        tips: vec![PathBuf::from(TIPS)],
        home: vec![
            PathBuf::from(HOME),
            PathBuf::from(r"C:\Users\OWNER~1"),
            PathBuf::from("%USERPROFILE%"),
        ],
        files: vec![
            CopiedFile::copied("answer.md", "answer.md"),
            CopiedFile::copied("my report.md", "my report.md"),
            CopiedFile::copied("报告.md", "报告.md"),
            CopiedFile::copied("sub/table.csv", "sub/table.csv"),
            CopiedFile::copied("result.md", "deliverable-result.md"),
            CopiedFile::refused("big.bin"),
            CopiedFile::refused("deliverable-result.md"),
        ],
    }
}

fn fwd() -> String {
    D.replace('\\', "/")
}

/// Each row, and (LOW 6) the rewrite is idempotent on it.
fn check(cases: Vec<(String, String)>) {
    let p = places();
    for (text, want) in cases {
        let once = relink(&text, &p);
        assert_eq!(once, want, "input: {text}");
        assert_eq!(relink(&once, &p), once, "not idempotent: {text}");
    }
}

fn row(text: String, want: &str) -> (String, String) {
    (text, want.to_string())
}

/// Paths into this round's deliverables folder: where the copy is.
#[test]
fn deliverable_paths_become_paths_in_the_tips_folder() {
    let f = fwd();
    let lower = format!("c{}", &f[1..]);
    check(vec![
        row(format!("Saved to `{f}/answer.md`."), "Saved to `agent/survey/answer.md`."),
        row(format!("Saved to {D}\\answer.md, done"), "Saved to agent/survey/answer.md, done"),
        row(format!("see {lower}/sub/table.csv"), "see agent/survey/sub/table.csv"),
        row(format!("[the answer]({f}/answer.md)"), "[the answer](agent/survey/answer.md)"),
        row(format!("file:///{f}/answer.md"), "agent/survey/answer.md"),
        row(format!(r"\\?\{D}\answer.md"), "agent/survey/answer.md"),
        row(format!("all in {f}/"), "all in agent/survey/"),
        row(format!("all in {f}."), "all in agent/survey/."),
        row(format!("{}\\\\answer.md", D.replace('\\', "\\\\")), "agent/survey/answer.md"),
        // Renamed on copy: pointed at its new name.
        row(format!("{f}/result.md"), "agent/survey/deliverable-result.md"),
        // Refused (here: its name was taken on copy): named, never pointed at.
        row(format!("{f}/big.bin"), "big.bin (not copied)"),
        row(format!("{f}/deliverable-result.md"), "deliverable-result.md (not copied)"),
        // Unknown: its name, no claim either way.
        row(format!("{f}/ghost.md"), "ghost.md"),
        // A folder of copied files.
        row(format!("{f}/sub"), "agent/survey/sub"),
        // Other spellings of this round's folder.
        row(
            r"%APPDATA%\claude-graph\inbox\rounds\survey-0123abcd\20261110-090000-ab12\nodes\task\deliverables\answer.md".into(),
            "agent/survey/answer.md",
        ),
        row(
            "~/AppData/Roaming/claude-graph/inbox/rounds/survey-0123abcd/20261110-090000-ab12/nodes/task/deliverables/answer.md".into(),
            "agent/survey/answer.md",
        ),
    ]);
}

/// M1: what follows a known file name is not part of it; names are matched
/// against the known files, not a character class.
#[test]
fn known_file_names_end_where_the_file_list_says() {
    let f = fwd();
    check(vec![
        row(format!("{f}/answer.md#L10"), "agent/survey/answer.md#L10"),
        row(format!("{f}/answer.md:12"), "agent/survey/answer.md:12"),
        row(
            format!("{f}/answer.md:12:3 x"),
            "agent/survey/answer.md:12:3 x",
        ),
        row(format!("{f}/报告.md。"), "agent/survey/报告.md。"),
        row(
            format!("{f}/answer.md，谢谢"),
            "agent/survey/answer.md，谢谢",
        ),
        row(format!("_{f}/answer.md_"), "_agent/survey/answer.md_"),
        row(format!("{f}/answer.md…"), "agent/survey/answer.md…"),
        row(
            format!("{f}/my report.md is ready"),
            "agent/survey/my report.md is ready",
        ),
        row(format!("{f}/my%20report.md"), "agent/survey/my report.md"),
        row(
            format!("{f}\\\\sub\\\\table.csv"),
            "agent/survey/sub/table.csv",
        ),
        row(format!("{f}/sub/./table.csv"), "agent/survey/sub/table.csv"),
        row(format!("{f}//sub//table.csv"), "agent/survey/sub/table.csv"),
        // A longer name that only starts like a known one is not it.
        row(format!("{f}/answer.md.bak"), "answer.md.bak"),
        // Unknown names: trailing punctuation and positions stay outside.
        row(format!("{f}/ghost.md:7."), "ghost.md:7."),
        row(format!("{f}/ghost.md#top"), "ghost.md#top"),
    ]);
}

/// L1: out of the folder again is not a deliverable.
#[test]
fn dot_dot_leaves_the_deliverables_folder() {
    let f = fwd();
    check(vec![
        row(format!("{f}/../brief.md"), ELSEWHERE),
        row(format!("{f}/.."), ELSEWHERE),
        row(format!("{f}/.... ok"), &format!("{ELSEWHERE} ok")),
    ]);
}

/// M2: another tip's or another round's folder is never this tip's results.
#[test]
fn other_rounds_are_never_pointed_at() {
    check(vec![
        row(
            format!("{DATA}\\inbox\\rounds\\other-99\\r\\nodes\\task\\deliverables\\answer.md"),
            ELSEWHERE,
        ),
        row(
            "~/AppData/Roaming/claude-graph/inbox/rounds/other-99/r/nodes/task/deliverables/answer.md".into(),
            ELSEWHERE,
        ),
        row(
            r"D:\Other\AppData\Roaming\claude-graph\inbox\rounds\survey-0123abcd\20261110-090000-ab12\nodes\task\deliverables\answer.md".into(),
            ELSEWHERE,
        ),
    ]);
}

#[test]
fn other_local_paths_become_a_fixed_phrase() {
    let round_fwd = ROUND.replace('\\', "/");
    check(vec![
        row(
            format!("read the brief at {round_fwd}/nodes/task/brief.md and"),
            &format!("read the brief at {ELSEWHERE} and"),
        ),
        row(
            format!("the round folder `{ROUND}`"),
            &format!("the round folder `{ELSEWHERE}`"),
        ),
        row(format!("{DATA}\\settings.json"), ELSEWHERE),
        row(
            r"%APPDATA%\claude-graph\inbox\rounds\x-1\r1\nodes\task\brief.md".into(),
            ELSEWHERE,
        ),
        row(
            r"D:\Other\AppData\Roaming\claude-graph\settings.json".into(),
            ELSEWHERE,
        ),
        // L2: text glued before the path stays.
        row(
            format!("文件在{DATA}\\settings.json"),
            &format!("文件在{ELSEWHERE}"),
        ),
        row(format!("path={ROUND}\\x"), &format!("path={ELSEWHERE}")),
        // L5: a verbatim prefix goes with the path.
        row(format!(r"\\?\{ROUND}\x"), ELSEWHERE),
        row(format!(r"\\.\{DATA}\x"), ELSEWHERE),
    ]);
}

/// M4: the tips folder becomes relative; the home folder becomes `~`;
/// anything else is left alone.
#[test]
fn the_tips_folder_and_the_home_folder_are_not_shown() {
    check(vec![
        row(
            format!(r"{TIPS}\agent\survey\answer.md"),
            "agent/survey/answer.md",
        ),
        row(format!(r"{TIPS}\notes.md."), "notes.md."),
        row(format!("in {TIPS}"), "in (the tips folder)"),
        row(
            r"C:\Users\Owner\AppData\Local\Temp\x.txt".into(),
            "~/AppData/Local/Temp/x.txt",
        ),
        row(
            r"c:/users/owner/.claude/settings.json".into(),
            "~/.claude/settings.json",
        ),
        row(
            r"C:\Users\Owner\Documents\paper.pdf is yours".into(),
            "~/Documents/paper.pdf is yours",
        ),
        // N2: a variable-rooted home path is left as written.
        row(
            r"%USERPROFILE%\Documents\x.md".into(),
            r"%USERPROFILE%\Documents\x.md",
        ),
        row(r"C:\Users\Owner".into(), "~"),
        // Not under home: left alone.
        row(r"D:\data\x.csv".into(), r"D:\data\x.csv"),
        // Only a prefix of the home folder's last name: not home.
        row(r"C:\Users\Owner2\x.md".into(), r"C:\Users\Owner2\x.md"),
    ]);
}

#[test]
fn text_without_such_paths_is_unchanged() {
    for text in [
        "",
        "Found 14 papers; the table is in table.csv.",
        "agent/survey/answer.md already relative",
        "claude-graph is the app",
        "调研 ⭐ done",
        // L4: prose about the layout, no root.
        "rounds live in claude-graph/inbox/rounds/<tip>/<run>/",
        "nodes/task/deliverables/answer.md",
    ] {
        assert_eq!(relink(text, &places()), text);
    }
}

/// L3: a user name with a space is matched whole, never half shown.
#[test]
fn a_path_with_spaces_in_the_user_name_is_matched_whole() {
    let home = PathBuf::from(r"C:\Users\John Smith");
    let round = home.join(r"AppData\Roaming\claude-graph\inbox\rounds\t-1\r1");
    let p = Places {
        tip_id: "t".into(),
        deliverables: vec![round.join("nodes").join("task").join("deliverables")],
        private: vec![round.clone()],
        tips: Vec::new(),
        home: vec![home],
        files: vec![CopiedFile::copied("a.md", "a.md")],
    };
    let text = r"wrote C:/Users/John Smith/AppData/Roaming/claude-graph/inbox/rounds/t-1/r1/nodes/task/deliverables/a.md ok";
    assert_eq!(relink(text, &p), "wrote agent/t/a.md ok");
    assert_eq!(
        relink(r"C:\Users\John Smith\Desktop\x.txt", &p),
        "~/Desktop/x.txt"
    );
    assert_eq!(
        relink(
            r"C:\Users\John Smith\AppData\Roaming\claude-graph\settings.json",
            &p
        ),
        ELSEWHERE
    );
}

/// HIGH 2: Git-Bash and WSL spellings of a drive; never inside URLs.
#[test]
fn git_bash_and_wsl_drive_spellings_are_drives() {
    check(vec![
        row("/c/Users/Owner/Documents/x".into(), "~/Documents/x"),
        row("/mnt/c/Users/Owner/Documents/x".into(), "~/Documents/x"),
        row("see /C/Users/Owner/x.".into(), "see ~/x."),
        row("/C:/Users/Owner/Documents/x".into(), "~/Documents/x"),
        row(
            "/c/Users/Owner/AppData/Roaming/claude-graph/inbox/rounds/survey-0123abcd/20261110-090000-ab12/nodes/task/deliverables/answer.md".into(),
            "agent/survey/answer.md",
        ),
        // Inside a URL `/c/` is no drive (the backstop hides the name).
        row(
            "https://x.org/c/Users/Owner/x".into(),
            &format!("https://x.org/c/Users/{ELSEWHERE}/x"),
        ),
        row("https://x.org/c/Docs/x".into(), "https://x.org/c/Docs/x"),
        row("a//c/Docs/x".into(), "a//c/Docs/x"),
    ]);
}

/// MEDIUM 3: percent-encoded roots (VS Code URIs); LOW 7: PowerShell.
#[test]
fn encoded_and_shell_spellings_of_a_root() {
    check(vec![
        row("file:///c%3A/Users/Owner/x".into(), "~/x"),
        row("C%3A%5CUsers%5COwner".into(), "~"),
        row("file:///C:/Users/Own%65r/x".into(), "~/x"),
        row("file://localhost/C:/Users/Owner/x".into(), "~/x"),
        row(r"$env:APPDATA\claude-graph\inbox\x".into(), ELSEWHERE),
        // N2: variable-rooted home paths stay as written (copied commands
        // keep working); they still resolve to the folders below.
        row(
            r"$env:USERPROFILE\Documents\x".into(),
            r"$env:USERPROFILE\Documents\x",
        ),
        row("$HOME/Documents/x".into(), "$HOME/Documents/x"),
        row(r#"cd "$HOME/proj""#.into(), r#"cd "$HOME/proj""#),
        row("$USERPROFILE/x".into(), "$USERPROFILE/x"),
        row(
            r"%USERPROFILE%\AppData\Roaming\claude-graph\inbox\rounds\survey-0123abcd\20261110-090000-ab12\nodes\task\deliverables\answer.md".into(),
            "agent/survey/answer.md",
        ),
        row(
            r"%USERPROFILE%\OneDrive\Vindictive\notes.md".into(),
            "notes.md",
        ),
        row("$HOME/AppData/Roaming/claude-graph".into(), ELSEWHERE),
        row(r"%appdata%\Claude-Graph\x".into(), ELSEWHERE),
    ]);
}

/// MEDIUM 4: a drive letter never ends a word or follows a colon.
#[test]
fn drive_letters_inside_words_and_urls_are_not_drives() {
    check(vec![
        row(
            "https://example.com/claude-graph/inbox/x".into(),
            "https://example.com/claude-graph/inbox/x",
        ),
        row("https://example.com/~/x".into(), "https://example.com/~/x"),
        // Not a drive; the backstop still hides the user name after `Users`.
        row(
            "abC:/Users/Owner/x".into(),
            &format!("abC:/Users/{ELSEWHERE}/x"),
        ),
        row("ratio C:/N is 3:/4".into(), "ratio C:/N is 3:/4"),
    ]);
}

/// MEDIUM 5: a folder of copies counts only as the whole path; a file under
/// it that was not copied is a name, no claim.
#[test]
fn a_file_under_a_folder_of_copies_is_not_pointed_at() {
    let f = fwd();
    check(vec![
        row(format!(r"{D}\sub\nope.csv"), "sub/nope.csv"),
        row(format!("{f}/sub/"), "agent/survey/sub/"),
        row(format!("{f}/sub."), "agent/survey/sub."),
    ]);
}

/// LOW 6: 8.3 and long names mixed in one path.
#[test]
fn mixed_short_and_long_names_are_one_folder() {
    check(vec![
        row(
            r"C:\Users\OWNER~1\AppData\Roaming\claude-graph\inbox\rounds\survey-0123abcd\20261110-090000-ab12\nodes\task\deliverables\answer.md".into(),
            "agent/survey/answer.md",
        ),
        row(
            r"C:\Users\OWNER~1\AppData\Roaming\CLAUDE~1\inbox\rounds\SURVEY~1\20261~1\nodes\task\deliverables\answer.md".into(),
            "agent/survey/answer.md",
        ),
        row(r"C:\Users\OWNER~1\Documents\Tips\a.md".into(), "~/Documents/Tips/a.md"),
        row(r"C:\Users\Owner\AppData\Roaming\CLAUDE~1\settings.json".into(), ELSEWHERE),
    ]);
}

/// LOW 8: the home folder ends at a component boundary (the backstop then
/// hides the name).
#[test]
fn the_home_folder_ends_at_a_component_boundary() {
    check(vec![
        row(
            r"C:\Users\Owner~1x\x".into(),
            &format!(r"C:\Users\{ELSEWHERE}~1x\x"),
        ),
        row(
            r"C:\Users\Owner's files".into(),
            &format!(r"C:\Users\{ELSEWHERE}'s files"),
        ),
        row(r"C:\Users\Owner.\Documents\x".into(), "~/Documents/x"),
        row(r"C:\Users\Owner-old\x".into(), r"C:\Users\Owner-old\x"),
    ]);
}

/// LOW 7: the backstop. After the rewrite, `Users` + separator-like or
/// markup characters + the home folder's name, and `AppData` + … +
/// `Roaming` + … + `claude-graph`, never stay; the bare name in prose does.
#[test]
fn the_backstop_catches_odd_spellings_and_leaves_prose() {
    let n = ELSEWHERE;
    check(vec![
        row(
            r"\\localhost\C$\Users\Owner\x".into(),
            &format!(r"\\localhost\C$\Users\{n}\x"),
        ),
        row(r"C:Users\Owner\x".into(), &format!(r"C:Users\{n}\x")),
        row(
            "C:\\Users\\Own\u{200b}er\\x".into(),
            &format!(r"C:\Users\{n}\x"),
        ),
        row("C:＼Users＼Owner".into(), &format!("C:＼Users＼{n}")),
        row(
            "C:\u{2215}Users\u{2215}Owner".into(),
            &format!("C:\u{2215}Users\u{2215}{n}"),
        ),
        row(
            r"C:\Users\**Owner**\x".into(),
            &format!(r"C:\Users\**{n}**\x"),
        ),
        row(r"`C:\Users\`Owner".into(), &format!(r"`C:\Users\`{n}")),
        row(
            "C:&#92;Users&#92;Owner".into(),
            &format!("C:&#92;Users&#92;{n}"),
        ),
        row("C:\\Users\\Own\ner\\x".into(), &format!(r"C:\Users\{n}\x")),
        row(
            "C:\\Users\n\\Owner\\x".into(),
            &format!("C:\\Users\n\\{n}\\x"),
        ),
        row(
            "x AppData＼Roaming＼claude-graph y".into(),
            &format!("x {n} y"),
        ),
        row(
            "Thanks, Owner. Users like Owner agree.".into(),
            "Thanks, Owner. Users like Owner agree.",
        ),
    ]);
}

/// HIGH 1: linear time. Before the fix 64 KiB of `C:/` took 21.5 s and
/// `~/x` 24.6 s in a release build (quadratic: 1 MiB would take hours).
#[test]
fn the_rewrite_is_linear_on_adversarial_text() {
    let p = places();
    let limit = std::time::Duration::from_secs(2);
    for unit in ["C:/", "~/x", "%a%", "https://x;", "/c/", "$env:"] {
        let text = unit.repeat(1024 * 1024 / unit.len());
        let t0 = std::time::Instant::now();
        let out = relink(&text, &p);
        let took = t0.elapsed();
        assert_eq!(out, text, "{unit}");
        eprintln!("relink 1 MiB of {unit:?}: {took:?}");
        assert!(took < limit, "{unit}: {took:?}");
    }
    let long = [
        format!("C:{}", "/".repeat(1024 * 1024)),
        format!(r"C:\Users\Owner\{}", "a".repeat(1024 * 1024)),
        format!("{D}{}", r"\.".repeat(512 * 1024)),
    ];
    for text in long {
        let t0 = std::time::Instant::now();
        relink(&text, &p);
        assert!(t0.elapsed() < limit, "{:?}", t0.elapsed());
    }
}

/// N1: the 8.3 wildcard only joins a folder with its own short form; a
/// folder next to the home folder (here `C:\Users\Public\Tips`) never gets
/// a `~` spelling.
#[test]
fn an_8_3_home_name_never_matches_another_folder() {
    let p = Places {
        tip_id: "t".into(),
        deliverables: Vec::new(),
        private: Vec::new(),
        tips: vec![PathBuf::from(r"C:\Users\Public\Tips")],
        home: vec![
            PathBuf::from(r"C:\Users\ZhangWentao"),
            PathBuf::from(r"C:\Users\ZHANGW~1"),
        ],
        files: Vec::new(),
    };
    let rows = [
        ("~/Tips/a.md", "~/Tips/a.md"),
        (r"C:\Users\ZhangWentao\Tips\a.md", "~/Tips/a.md"),
        (r"C:\Users\ZHANGW~1\Tips\a.md", "~/Tips/a.md"),
        (r"C:\Users\Public\Tips\a.md", "a.md"),
        (
            r"C:\Users\PUBLIC~1\Tips\a.md",
            r"C:\Users\PUBLIC~1\Tips\a.md",
        ),
    ];
    for (text, want) in rows {
        let once = relink(text, &p);
        assert_eq!(once, want, "input: {text}");
        assert_eq!(relink(&once, &p), once, "not idempotent: {text}");
    }
}

/// N3: an invalid byte (passed in as a sentinel by the caller) inside a
/// path does not hide it from the rewrite or the backstop.
#[test]
fn a_sentinel_inside_a_path_is_skipped() {
    let s = sentinel(0);
    check(vec![
        row(format!(r"C:\Users\Own{s}er\x"), "~/x"),
        row(format!(r"C:\Users\{s}Owner\x"), "~/x"),
        row(
            format!("C:＼Users＼Own{s}er"),
            &format!("C:＼Users＼{ELSEWHERE}"),
        ),
        row(format!("keep {s} here"), &format!("keep {s} here")),
    ]);
}

/// N5: a domain profile folder (`<name>.<DOMAIN>`) hides the name; a file
/// named like the user does not; a drive after `Word:` is a drive.
#[test]
fn domain_profiles_and_drives_after_a_label() {
    let f = fwd();
    check(vec![
        row(
            r"C:\Users\Owner.CORP\x".into(),
            &format!(r"C:\Users\{ELSEWHERE}.CORP\x"),
        ),
        row("see Users/Owner.txt".into(), "see Users/Owner.txt"),
        row(r"Path:C:\Users\Owner\x".into(), "Path:~/x"),
        row(format!("key:{f}/answer.md"), "key:agent/survey/answer.md"),
        row("3:C:/x".into(), "3:C:/x"),
        row("x://C:/y".into(), "x://C:/y"),
    ]);
}
