from tools.transcript import redact


def test_redacts_github_tokens():
    text = "token ghp_abcdefghijklmnopqrstuv1234567890 done"
    assert "ghp_" not in redact.redact(text)
    assert "[redacted:github-token]" in redact.redact(text)


def test_redacts_api_keys_and_aws():
    text = "sk-abcdefghijklmnop1234 and AKIAABCDEFGHIJKLMNOP"
    result = redact.redact(text)
    assert "[redacted:api-key]" in result
    assert "[redacted:aws-key]" in result


def test_redacts_key_value_pairs():
    result = redact.redact("export API_KEY=supersecretvalue123")
    assert "supersecretvalue123" not in result
    assert "API_KEY" in result


def test_redacts_private_key_blocks():
    text = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----"
    assert redact.redact(text) == "[redacted:private-key]"


def test_leaves_normal_text_alone():
    text = "git commit -m 'update config' && git push origin main"
    assert redact.redact(text) == text


def test_passthrough_never_changes():
    text = "ghp_abcdefghijklmnopqrstuv1234567890"
    assert redact.passthrough(text) == text
