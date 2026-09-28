#[cfg(target_os = "macos")]
pub fn approve(reason: &str) -> Result<(), String> {
    use std::sync::mpsc;
    use std::time::Duration;

    use robius_authentication::{
        AndroidText, BiometricStrength, Context, PolicyBuilder, Text, WindowsText,
    };

    const WAIT: Duration = Duration::from_secs(60);

    let policy = PolicyBuilder::new()
        .biometrics(Some(BiometricStrength::Strong))
        .password(false)
        .companion(false)
        .build()
        .ok_or("Touch ID policy unavailable")?;
    let text = Text {
        android: AndroidText {
            title: reason,
            subtitle: None,
            description: None,
        },
        apple: reason,
        windows: WindowsText::new_truncated(reason, reason),
    };
    let (sender, receiver) = mpsc::channel();
    Context::new(())
        .authenticate(text, &policy, move |result| {
            let _ = sender.send(result);
        })
        .map_err(|error| format!("Touch ID: {error:?}"))?;
    match receiver.recv_timeout(WAIT) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("Touch ID: {error:?}")),
        Err(_) => Err("Touch ID: no answer".to_string()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn approve(_: &str) -> Result<(), String> {
    Err("Touch ID needs macOS".to_string())
}
