//! Explicit live smoke test of the same native transport used by Kritic.
use std::io::Read;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::args()
        .nth(1)
        .ok_or("Pass the official Codex executable path")?;
    let mut original = String::new();
    std::io::stdin().take(65537).read_to_string(&mut original)?;
    let _ = kritic_prompts::evaluate(&original, None)?;
    let packet=serde_json::json!({"response_schema":kritic_prompts::schema(),"untrusted_user_prompt":original}).to_string();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        kritic_prompts::codex::generate(std::path::Path::new(&executable), "", &packet),
    )
    .await??;
    println!(
        "{}",
        serde_json::to_string_pretty(&kritic_prompts::evaluate(&original, Some(response))?)?
    );
    Ok(())
}
