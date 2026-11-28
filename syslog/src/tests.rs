use super::*;

fn lines_of(lines: &mut Lines, pieces: &[(Source, u64, &str)]) -> Vec<Line> {
    let mut out = Vec::new();
    for &(source, time, text) in pieces {
        lines.piece(source, time, text.as_bytes(), &mut out);
    }
    out
}

#[test]
fn each_writers_pieces_are_put_back_into_its_own_lines() {
    let mut lines = Lines::new();
    lines.started(7, "chello");
    let out = lines_of(
        &mut lines,
        &[
            (Source::Program(7), 100, "argc "),
            (Source::Kernel, 200, "[process] 8 \"hello\" exited with code 0\n"),
            (Source::Program(7), 300, "1\nargv[0] "),
            (Source::Program(7), 400, "chello\n"),
        ],
    );
    let got: Vec<(&str, u64, &str)> = out.iter().map(|l| (l.source.as_str(), l.time, l.text.as_str())).collect();
    assert_eq!(
        got,
        [("kernel", 200, "[process] 8 \"hello\" exited with code 0"), ("chello", 100, "argc 1"), ("chello", 300, "argv[0] chello")]
    );
}

#[test]
fn a_program_not_seen_starting_is_named_by_its_id_and_a_name_can_be_reused() {
    let mut lines = Lines::new();
    let out = lines_of(&mut lines, &[(Source::Program(9), 1, "hi\n")]);
    assert_eq!(out[0].source, "pid 9");
    lines.started(9, "first");
    lines.started(9, "second"); // the id given to another program
    let out = lines_of(&mut lines, &[(Source::Program(9), 2, "hi\n")]);
    assert_eq!(out[0].source, "second");
    for pid in 0..300 {
        lines.started(1000 + pid, "many");
    }
    assert_eq!(lines.names.len(), NAMES_MAX);
}

#[test]
fn a_line_left_open_comes_out_once_it_is_old_and_a_long_one_is_cut() {
    let mut lines = Lines::new();
    let mut out = lines_of(&mut lines, &[(Source::Program(3), 1_000, "> ")]);
    assert!(out.is_empty());
    lines.flush_older(5_000, &mut out);
    assert!(out.is_empty(), "not old yet");
    let _ = lines_of(&mut lines, &[(Source::Kernel, 10_000, "[kernel] idle\n")]);
    lines.flush_older(5_000, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].text, "> ");
    let long = "x".repeat(LINE_MAX + 10) + "\n";
    let out = lines_of(&mut lines, &[(Source::Kernel, 1, &long)]);
    assert_eq!(out.iter().map(|l| l.text.len()).collect::<Vec<_>>(), [LINE_MAX, 10]);
}

#[test]
fn too_many_open_lines_let_the_oldest_go() {
    let mut lines = Lines::new();
    let mut out = Vec::new();
    for pid in 0..OPEN_MAX as u32 + 1 {
        lines.piece(Source::Program(pid), u64::from(pid) + 1, b"open", &mut out);
    }
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].source, "pid 0");
}

#[test]
fn control_characters_do_not_reach_the_file() {
    let mut lines = Lines::new();
    let out = lines_of(&mut lines, &[(Source::Kernel, 1, "a\r\x1b[1mb\tc\n")]);
    assert_eq!(out[0].text, "a?[1mb\tc");
    let mut out = Vec::new();
    lines.piece(Source::Kernel, 1, &[0xff, b'\n'], &mut out);
    assert_eq!(out[0].text, "\u{fffd}");
}

#[test]
fn the_level_is_read_from_the_text() {
    assert_eq!(level("KERNEL PANIC at x"), Level::Crit);
    assert_eq!(level("THE SYSTEM HAS STOPPED: init exited with code 42"), Level::Crit);
    assert_eq!(level("[ahci] the controller would not start: no disk is attached"), Level::Error);
    assert_eq!(level("[init] ahci ended (code 5) 5 times in 30 s - giving up on it"), Level::Error);
    assert_eq!(level("FAIL: something"), Level::Error);
    assert_eq!(level("[init] ahci ended with code 5 - restarting it"), Level::Warn);
    assert_eq!(level("[init] no driver for PCI device 1234:1111"), Level::Warn);
    assert_eq!(level("[kernel] idle"), Level::Info);
    assert!(Level::Crit > Level::Error && Level::Error > Level::Warn && Level::Warn > Level::Info);
    for level in [Level::Info, Level::Warn, Level::Error, Level::Crit] {
        assert_eq!(Level::parse(level.name()), Some(level));
    }
    assert_eq!(Level::parse("loud"), None);
}

#[test]
fn dates_are_utc_to_the_millisecond() {
    assert_eq!(date(1), "1970-01-01 00:00:00.000");
    // 2026-11-28 12:34:56.789 UTC
    assert_eq!(date(1_795_869_296_789_000_000), "2026-11-28 12:34:56.789");
    // A leap day, and the turn of a century that is not a leap year.
    assert_eq!(date(951_782_400_000_000_000), "2000-02-29 00:00:00.000");
    assert_eq!(date(4_107_542_399_000_000_000), "2100-02-28 23:59:59.000");
    assert_eq!(date(0), "early boot             ");
    assert_eq!(date(0).len(), DATE_WIDTH);
}

#[test]
fn a_formatted_line_is_parsed_back() {
    let line = Line { time: 1_795_869_296_789_000_000, source: "ahci".into(), level: Level::Error, text: "[ahci] would not start".into() };
    let text = format(&line);
    assert_eq!(text, "2026-11-28 12:34:56.789 ahci       error [ahci] would not start\n");
    assert_eq!(parse(&text), Some(("2026-11-28 12:34:56.789", "ahci", Level::Error, "[ahci] would not start")));
    let line = Line { time: 0, source: "pid 12".into(), level: Level::Info, text: "hello".into() };
    assert_eq!(parse(&format(&line)), Some(("early boot", "pid 12", Level::Info, "hello")));
    let line = Line { time: 5, source: "averyverylongname".into(), level: Level::Warn, text: String::new() };
    assert_eq!(parse(&format(&line)), Some(("1970-01-01 00:00:00.000", "averyverylongname", Level::Warn, "")));
    assert_eq!(parse("not a line"), None);
    assert_eq!(parse("2026-11-28 12:34:56.789 kernel     loud  x"), None);
}
