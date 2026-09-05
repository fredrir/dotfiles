use super::*;

fn strings(session: &Session) -> Vec<String> {
    session
        .args()
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_cabled_address_is_the_cable() {
    assert_eq!(
        classify("hostname 10.77.77.2\nuser fredrir\n"),
        Some(Route::Cable)
    );
}

#[test]
fn a_direct_wifi_address_is_not_mistaken_for_the_cable() {
    assert_eq!(classify("hostname 10.77.78.2\n"), Some(Route::Wifi));
}

#[test]
fn the_filtered_lan_is_named_by_its_proxy_rather_than_its_address() {
    let config = "hostname archie.local\nproxycommand /home/f/.ssh/bin/home-lan-connect %h %p\n";
    assert_eq!(classify(config), Some(Route::Lan));
}

#[test]
fn anything_else_resolved_is_tailscale() {
    assert_eq!(classify("hostname archie\n"), Some(Route::Tailscale));
    assert_eq!(
        classify("hostname 100.126.231.24\n"),
        Some(Route::Tailscale)
    );
}

#[test]
fn a_config_with_no_hostname_resolves_to_no_route() {
    assert_eq!(classify(""), None);
    assert_eq!(classify("user fredrir\nport 22\n"), None);
    assert_eq!(parse("").unwrap_err(), "ssh -G returned no hostname");
}

#[test]
fn a_valueless_line_is_skipped_rather_than_abandoning_the_config() {
    let config = "channeltimeout\nhostname 10.77.77.2\nlogleveldebug\n";
    assert_eq!(classify(config), Some(Route::Cable));
}

#[test]
fn a_resolved_config_carries_every_field_a_caller_reads() {
    let resolved = parse(concat!(
        "user fredrir\n",
        "hostname 100.126.231.24\n",
        "port 2222\n",
        "bindaddress 10.77.77.1\n",
        "bindinterface en11\n",
        "proxycommand /home/f/.ssh/bin/home-lan-connect %h %p\n",
        "controlpath /tmp/ssh-archie\n",
    ))
    .unwrap();
    assert_eq!(resolved.hostname, "100.126.231.24");
    assert_eq!(resolved.user.as_deref(), Some("fredrir"));
    assert_eq!(resolved.port, Some(2222));
    assert_eq!(resolved.bound.as_deref(), Some("en11"));
    assert_eq!(
        resolved.proxy.as_deref(),
        Some("/home/f/.ssh/bin/home-lan-connect %h %p")
    );
    assert_eq!(resolved.control_path.as_deref(), Some("/tmp/ssh-archie"));
    assert_eq!(resolved.route, Route::Lan);
}

#[test]
fn a_bind_address_is_only_read_when_no_interface_is_bound() {
    let resolved = parse("hostname archie\nbindaddress 10.77.77.1\nbindinterface none\n").unwrap();
    assert_eq!(resolved.bound.as_deref(), Some("10.77.77.1"));
}

#[test]
fn the_word_none_is_an_absent_field_rather_than_a_value() {
    let resolved = parse(concat!(
        "hostname archie\n",
        "bindaddress none\n",
        "bindinterface none\n",
        "proxycommand none\n",
        "controlpath none\n",
        "port not-a-port\n",
    ))
    .unwrap();
    assert_eq!(resolved.bound, None);
    assert_eq!(resolved.proxy, None);
    assert_eq!(resolved.control_path, None);
    assert_eq!(resolved.port, None);
    assert_eq!(resolved.route, Route::Tailscale);
}

#[test]
fn a_session_has_no_tty_and_keeps_the_script_one_argument() {
    assert_eq!(
        strings(&Session::new("archie").script("test -d '/a b'")),
        [
            "-T",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "LogLevel=ERROR",
            "--",
            "archie",
            "test -d '/a b'",
        ]
    );
}

#[test]
fn an_interactive_session_allocates_a_tty_for_the_far_side() {
    assert_eq!(
        strings(
            &Session::new("macie")
                .interactive()
                .script("exec zsh -lic 'codex'")
        ),
        [
            "-tt",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "LogLevel=ERROR",
            "--",
            "macie",
            "exec zsh -lic 'codex'",
        ]
    );
}

#[test]
fn a_session_without_a_script_is_a_plain_connection() {
    assert_eq!(
        strings(&Session::new("archie")),
        [
            "-T",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "LogLevel=ERROR",
            "--",
            "archie"
        ]
    );
}

#[test]
fn a_batch_session_cannot_prompt_and_gives_up_on_a_dead_link() {
    let arguments = strings(&Session::new("archie").batch().script("exec agent-hop"));
    assert_eq!(arguments[0], "-T");
    for option in [
        "ConnectTimeout=8",
        "LogLevel=ERROR",
        "BatchMode=yes",
        "ConnectionAttempts=1",
        "ServerAliveInterval=5",
        "ServerAliveCountMax=3",
    ] {
        assert!(arguments.iter().any(|value| value == option), "{option}");
    }
    assert_eq!(
        arguments[arguments.len() - 3..],
        ["--", "archie", "exec agent-hop"]
    );
}

#[test]
fn the_host_is_separated_from_the_options_so_a_dash_stays_a_host() {
    let arguments = strings(&Session::new("-oProxyCommand=reboot"));
    assert_eq!(arguments[arguments.len() - 2], "--");
    assert_eq!(arguments[arguments.len() - 1], "-oProxyCommand=reboot");
}

#[test]
fn an_extra_option_is_added_once_and_after_the_canonical_ones() {
    let session = Session::new("archie")
        .option("RequestTTY=no")
        .option("RequestTTY=no")
        .option("ConnectTimeout=8");
    assert_eq!(
        strings(&session),
        [
            "-T",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "LogLevel=ERROR",
            "-o",
            "RequestTTY=no",
            "--",
            "archie",
        ]
    );
}

#[test]
fn an_option_replaces_the_canonical_one_it_repeats_rather_than_being_ignored() {
    assert_eq!(
        strings(&Session::new("archie").option("ConnectTimeout=2")),
        [
            "-T",
            "-o",
            "ConnectTimeout=2",
            "-o",
            "LogLevel=ERROR",
            "--",
            "archie",
        ]
    );
}

#[test]
fn the_rsync_transport_dials_on_the_same_terms_as_a_session() {
    assert_eq!(transport(), "ssh -o ConnectTimeout=8 -o LogLevel=ERROR");
    for option in OPTIONS {
        assert!(transport().contains(&format!("-o {option}")));
    }
}

#[test]
fn a_home_directory_is_one_absolute_line() {
    assert_eq!(
        parse_home("archie", b"/home/fredrir\n").unwrap(),
        PathBuf::from("/home/fredrir")
    );
    assert_eq!(
        parse_home("archie", b"/home/fred rir\r\n").unwrap(),
        PathBuf::from("/home/fred rir")
    );
    assert_eq!(
        parse_home("archie", b"/home/fredrir").unwrap(),
        PathBuf::from("/home/fredrir")
    );
}

#[test]
fn a_home_directory_that_is_not_one_absolute_line_is_refused() {
    for bytes in [
        &b""[..],
        &b"\n"[..],
        &b"relative/home\n"[..],
        &b"/home/a\n/home/b\n"[..],
        &b"/home/a\rnope"[..],
        &b"\xff/home\n"[..],
    ] {
        let error = parse_home("archie", bytes).unwrap_err();
        assert!(error.starts_with("archie returned a"), "{error}");
    }
}

#[test]
fn a_missing_ssh_is_named_rather_than_reported_as_an_io_error() {
    assert_eq!(
        command_error(io::Error::from(io::ErrorKind::NotFound)),
        "ssh is required"
    );
    assert!(command_error(io::Error::from(io::ErrorKind::PermissionDenied)).starts_with("ssh: "));
}

#[test]
fn a_failure_is_reported_by_its_first_useful_stderr_line() {
    assert_eq!(stderr_reason(b"  \n no route \n", "fallback"), "no route");
    assert_eq!(stderr_reason(b"", "fallback"), "fallback");
    assert_eq!(stderr_reason(b"   ", "fallback"), "fallback");
}
