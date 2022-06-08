use std::{
    env, fs,
    path::{Path, PathBuf}
};

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use indoc::formatdoc;
use jwt_simple::prelude::*;
use reqwest::{
    blocking as http,
    header::{ACCEPT, USER_AGENT},
    Url
};
use serde_json::Value as JsonValue;

// -----------------------------------------------------------------------------
// Command line parsing
// -----------------------------------------------------------------------------

/// Generate an access token for a GitHub App installation.
#[derive(Parser, Debug)]
#[clap(about)]
struct Opts {

    /// The GitHub App ID
    #[clap(short = 'a', long, env = "GITHUB_APP_ID", conflicts_with = "app-id-file")]
    app_id: Option<String>,

    /// The path to a file containing GitHub App ID
    #[clap(short = 'A', long, env = "GITHUB_APP_ID_FILE", default_value = "app-id")]
    app_id_file: String,

    /// The GitHub App private key, in PEM format
    #[clap(short = 'k', long, env = "GITHUB_APP_PRIVATE_KEY", conflicts_with = "private-key-file")]
    private_key: Option<String>,

    /// The path a file containing the GitHub App private key, in PEM format
    #[clap(short = 'K', long, env = "GITHUB_APP_PRIVATE_KEY_FILE", default_value = "private-key.pem")]
    private_key_file: String,

    /// The GitHub App ID
    #[clap(short = 'i', long, env = "GITHUB_APP_INSTALLATION_ID", conflicts_with = "installation-id-file")]
    installation_id: Option<String>,

    /// The path to a file containing GitHub App ID
    #[clap(short = 'I', long, env = "GITHUB_APP_INSTALLATION_ID_FILE", default_value = "installation-id")]
    installation_id_file: String,

    #[clap(flatten)]
    github: GithubOpts,

    #[clap(flatten)]
    output: OutputOpts,
}

#[derive(clap::Args, Debug)]
struct GithubOpts {

    /// The GitHub URL (used for writing Git basic auth config files)
    #[clap(long = "github-url", env = "GITHUB_URL", default_value = "https://github.com")]
    url: Url,

    /// The GitHub API URL (used for requesting the access token)
    #[clap(long = "github-api-url", env = "GITHUB_API_URL", default_value = "https://api.github.com")]
    api_url: Url,

}

#[derive(clap::Args, Debug)]
struct OutputOpts {
    /// Print the outcome to standard output
    #[clap(arg_enum, short, long, default_missing_value = "token")]
    print: Option<PrintStyle>,

    /// Write the token to the given file
    #[clap(short, long)]
    write_to: Option<PathBuf>,

    /// Write out a '.gitconfig' and '.git-credentials' file to the given directory
    #[clap(short = 'c', long, default_missing_value = "~")]
    git_config: Option<PathBuf>,

    /// Overwrite existing files
    #[clap(short, long)]
    force: bool,
}

#[derive(Debug, Clone, clap::ArgEnum)]
enum PrintStyle {
    /// Just print the token string
    Token,
    /// Print the entire JSON response from the GitHub 'access_token' request
    Response,
}

impl std::default::Default for PrintStyle {
    fn default() -> Self { PrintStyle::Token }
}

struct ParsedOpts {
    app_id: String,
    private_key: String,
    installation_id: String,
    github: GithubOpts,
    output: OutputOpts,
}

fn from_string_opt(description: &str, value: Option<String>, file_path: String) -> Result<String> {
    match value {
        Some(s) => Ok(s),
        None => fs::read_to_string(&file_path)
            .with_context(|| format!("Failed to read {} from file: {}", description, file_path))
    }
}

impl Opts {

    fn finish_parsing(self) -> Result<ParsedOpts> {
        Ok(ParsedOpts {
            app_id: from_string_opt("App ID", self.app_id, self.app_id_file)?,
            private_key: from_string_opt("private key", self.private_key, self.private_key_file)?,
            installation_id: from_string_opt("App installation ID", self.installation_id, self.installation_id_file)?,
            github: self.github,
            output: self.output,
        })
    }

}

// -- End Command-Line Parsing -------------------------------------------------

fn main() -> Result<()> {

    // Parse command line
    let opts = Opts::parse().finish_parsing()?;

    // Send token request to GitHub
    let response: JsonValue = http::Client::new()
        .post(opts.github.api_url.join(&format!("app/installations/{}/access_tokens", opts.installation_id))?)
        .header(USER_AGENT, &opts.app_id)
        .header(ACCEPT, "application/vnd.github.v3+json")
        .bearer_auth(generate_app_jwt(&opts.app_id, &opts.private_key)?)
        .send()?
        .error_for_status()?
        .json()?;

    // Extract token from response
    let token: String = response
        .get("token").and_then(JsonValue::as_str).map(String::from)
        .ok_or_else(|| anyhow!("Response from GitHub is missing the 'token' field: {}", response))?;

    // Write out to file (if requested)
    if let Some(ref file) = opts.output.write_to {
        write_file(file, &token, &opts.output)?;
    }

    // Write out .gitconfig and .git-credentials (if requested)
    if let Some(ref d) = opts.output.git_config {

        let dir: PathBuf =
            if d == &PathBuf::from("~") {
                PathBuf::from(env::var("HOME")
                    .map_err(|_| anyhow!("No path specified for Git config."))?)
            } else {
                PathBuf::from(d)
            };

        let mut url_with_credentials = opts.github.url.clone();
        url_with_credentials.set_username("git")
            .map_err(|_| anyhow!("Given GitHub URL is invalid: {}", opts.github.url))?;
        url_with_credentials.set_password(Some(&token))
            .map_err(|_| anyhow!("Given GitHub URL is invalid: {}", opts.github.url))?;

        fs::create_dir_all(&dir)?;

        write_file(
            dir.join(".gitconfig"),
            formatdoc!(r#"
                [credential "{}"]
                    helper= store
            "#, opts.github.url),
            &opts.output
        )?;

        write_file(
            dir.join(".git-credentials"),
            url_with_credentials.to_string(),
            &opts.output
        )?;
    }

    // Print to standard output (if requested)
    if let Some(ref style) = opts.output.print {
        match style {
            PrintStyle::Token => println!("{}", token),
            PrintStyle::Response => println!("{:#}", &response)
        }
    }

    Ok(())
}

/// Generates a GitHub App JWT for the given app ID, signed using the given key.
/// 
/// The generated JWT will have a 10-minute expiry.
fn generate_app_jwt(app_id: &str, private_key: &str) -> Result<String> {
    RS256KeyPair::from_pem(private_key)
        .context("Private key does not appear to be in PEM format.")?
        .sign(Claims::create(Duration::from_mins(9))
            .with_issuer(app_id)
        )
}

/// Write some content to a file, only if it doesn't already exist
/// (or `--force` was specified).
fn write_file<P,C>(path: P, content: C, opts: &OutputOpts) -> Result<()>
where
    P: AsRef<Path>,
    C: AsRef<[u8]>,
{
    let p: &Path = path.as_ref();

    if !opts.force && p.exists() {
        return Err(anyhow!("Refusing to overwrite existing {}", p.display()));
    }

    fs::write(p, content)?;

    Ok(())
}

