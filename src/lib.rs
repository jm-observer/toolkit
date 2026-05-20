use anyhow::{anyhow, bail, Context, Result};
use chrono::{NaiveDate, TimeZone, Utc};
use log::info;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Deserialize, Clone)]
pub struct Commit {
    pub sha: String,
    pub commit: CommitDetail,
    pub html_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CommitDetail {
    pub message: String,
    pub author: CommitAuthor,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CommitAuthor {
    pub name: String,
    pub email: String,
    pub date: String,
}

#[derive(Debug, Deserialize)]
pub struct RepoInfo {
    pub default_branch: String,
}

#[derive(Debug, Serialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub html_url: String,
}

impl From<Commit> for CommitInfo {
    fn from(c: Commit) -> Self {
        Self {
            sha: c.sha,
            message: c.commit.message,
            author: c.commit.author.name,
            email: c.commit.author.email,
            date: c.commit.author.date,
            html_url: c.html_url,
        }
    }
}

pub struct GitHubClient {
    client: Client,
    token: Option<String>,
}

impl GitHubClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("github-commit-info")
            .build()
            .context("构造 HTTP 客户端失败")?;
        let token = std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty());
        Ok(Self { client, token })
    }

    fn headers(&self) -> Result<reqwest::header::HeaderMap> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static("application/vnd.github.v3+json"),
        );
        if let Some(token) = &self.token {
            let value = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))
                .context("GITHUB_TOKEN 含非法字符，无法构造 Authorization 头")?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        }
        Ok(headers)
    }

    pub async fn get_default_branch(&self, owner: &str, repo: &str) -> Result<String> {
        let url = format!("https://api.github.com/repos/{}/{}", owner, repo);
        let response = self
            .client
            .get(&url)
            .headers(self.headers()?)
            .send()
            .await
            .context("请求仓库信息失败")?;
        if !response.status().is_success() {
            let status = response.status();
            if status.as_u16() == 403 {
                bail!("API请求被拒绝(403)。请设置 GITHUB_TOKEN 环境变量或检查 token 权限");
            }
            bail!("获取仓库信息失败: {}", status);
        }
        let repo_info: RepoInfo = response.json().await.context("解析仓库信息 JSON 失败")?;
        Ok(repo_info.default_branch)
    }

    /// 拉取指定时间窗口内的所有 commit，跟随 GitHub Link 头自动翻页。
    pub async fn fetch_commits(
        &self,
        owner: &str,
        repo: &str,
        branch: &str,
        since: &str,
        until: &str,
    ) -> Result<Vec<Commit>> {
        let first_url = format!("https://api.github.com/repos/{}/{}/commits", owner, repo);
        let initial_query: [(&str, &str); 4] = [
            ("sha", branch),
            ("since", since),
            ("until", until),
            ("per_page", "100"),
        ];

        let mut all = Vec::new();
        let mut next_url: Option<String> = Some(first_url);
        let mut first = true;
        while let Some(url) = next_url.take() {
            let mut req = self.client.get(&url).headers(self.headers()?);
            if first {
                req = req.query(&initial_query);
                first = false;
            }
            let response = req.send().await.context("请求 commits 失败")?;
            if !response.status().is_success() {
                let status = response.status();
                if status.as_u16() == 403 {
                    bail!("API请求被拒绝(403)。请设置 GITHUB_TOKEN 环境变量或检查 token 权限");
                }
                bail!("API请求失败: {}", status);
            }
            next_url = parse_next_link(response.headers().get(reqwest::header::LINK));
            let page: Vec<Commit> = response.json().await.context("解析 commits JSON 失败")?;
            all.extend(page);
        }
        Ok(all)
    }
}

/// 从 GitHub 仓库 URL 提取 (owner, repo)。
/// 接受 `https://github.com/owner/repo[.git][/]` 与 `git@github.com:owner/repo[.git]`。
pub fn parse_github_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches('/');
    let trimmed = trimmed.trim_end_matches(".git");
    let path = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("git@github.com:"))?;
    let mut iter = path.splitn(3, '/');
    let owner = iter.next()?;
    let repo = iter.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// 从 `Link` 头里取出 `rel="next"` 的 URL。
fn parse_next_link(link: Option<&reqwest::header::HeaderValue>) -> Option<String> {
    let header = link?.to_str().ok()?;
    for part in header.split(',') {
        let segments: Vec<&str> = part.split(';').map(str::trim).collect();
        if segments.len() < 2 {
            continue;
        }
        let url_seg = segments[0];
        if !url_seg.starts_with('<') || !url_seg.ends_with('>') {
            continue;
        }
        let url = &url_seg[1..url_seg.len() - 1];
        if segments[1..].contains(&"rel=\"next\"") {
            return Some(url.to_string());
        }
    }
    None
}

pub fn parse_start_date(date_str: &str) -> Result<String> {
    let naive_date = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .with_context(|| format!("日期格式错误: {}, 期望格式: yyyy-MM-dd", date_str))?;
    let datetime = naive_date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("日期解析失败"))?;
    Ok(Utc.from_utc_datetime(&datetime).to_rfc3339())
}

pub fn calculate_until_date(start_date: &str, days: i64) -> Result<String> {
    let naive_date = NaiveDate::parse_from_str(start_date, "%Y-%m-%d")
        .with_context(|| format!("日期格式错误: {}", start_date))?;
    let end_date = naive_date + chrono::Duration::days(days);
    let datetime = end_date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| anyhow!("日期解析失败"))?;
    Ok(Utc.from_utc_datetime(&datetime).to_rfc3339())
}

pub async fn run(url: &str, branch: Option<&str>, start_date: &str, days: i64) -> Result<()> {
    let client = GitHubClient::new()?;

    let (owner, repo) =
        parse_github_url(url).ok_or_else(|| anyhow!("无法从URL解析owner/repo: {}", url))?;

    let branch = if let Some(b) = branch {
        b.to_string()
    } else {
        info!("正在获取仓库默认分支...");
        client.get_default_branch(&owner, &repo).await?
    };

    let since = parse_start_date(start_date)?;
    let until = calculate_until_date(start_date, days)?;

    info!("仓库: {}/{}", owner, repo);
    info!("分支: {}", branch);
    info!(
        "时间范围: {} 至 {} (共{}天)",
        start_date,
        until.split('T').next().unwrap_or(&until),
        days
    );

    let commits = client
        .fetch_commits(&owner, &repo, &branch, &since, &until)
        .await?;

    if commits.is_empty() {
        info!("在指定时间范围内没有找到提交");
        return Ok(());
    }

    info!("找到 {} 个提交", commits.len());

    let commit_infos: Vec<CommitInfo> = commits.into_iter().map(CommitInfo::from).collect();
    let json = serde_json::to_string_pretty(&commit_infos).context("序列化 JSON 失败")?;

    println!("{}", json);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_start_date_basic() {
        let s = parse_start_date("2024-03-15").unwrap();
        assert_eq!(s, "2024-03-15T00:00:00+00:00");
    }

    #[test]
    fn parse_start_date_rejects_bad_format() {
        assert!(parse_start_date("2024/03/15").is_err());
        assert!(parse_start_date("not-a-date").is_err());
        assert!(parse_start_date("").is_err());
    }

    #[test]
    fn calculate_until_date_adds_days() {
        assert_eq!(
            calculate_until_date("2024-03-15", 7).unwrap(),
            "2024-03-22T00:00:00+00:00"
        );
    }

    #[test]
    fn calculate_until_date_zero_days_is_same_day() {
        assert_eq!(
            calculate_until_date("2024-03-15", 0).unwrap(),
            "2024-03-15T00:00:00+00:00"
        );
    }

    #[test]
    fn calculate_until_date_crosses_month() {
        assert_eq!(
            calculate_until_date("2024-02-28", 2).unwrap(),
            "2024-03-01T00:00:00+00:00"
        );
    }

    #[test]
    fn parse_github_url_https() {
        assert_eq!(
            parse_github_url("https://github.com/golang/go"),
            Some(("golang".into(), "go".into()))
        );
    }

    #[test]
    fn parse_github_url_handles_trailing_slash_and_git_suffix() {
        assert_eq!(
            parse_github_url("https://github.com/golang/go.git/"),
            Some(("golang".into(), "go".into()))
        );
        assert_eq!(
            parse_github_url("https://github.com/golang/go/"),
            Some(("golang".into(), "go".into()))
        );
    }

    #[test]
    fn parse_github_url_ssh() {
        assert_eq!(
            parse_github_url("git@github.com:golang/go.git"),
            Some(("golang".into(), "go".into()))
        );
    }

    #[test]
    fn parse_github_url_ignores_extra_path_segments() {
        assert_eq!(
            parse_github_url("https://github.com/golang/go/tree/master"),
            Some(("golang".into(), "go".into()))
        );
    }

    #[test]
    fn parse_github_url_rejects_non_github_hosts() {
        assert!(parse_github_url("https://gitlab.com/foo/bar").is_none());
        assert!(parse_github_url("https://example.com/github.com/foo/bar").is_none());
        assert!(parse_github_url("not a url").is_none());
        assert!(parse_github_url("https://github.com/").is_none());
        assert!(parse_github_url("https://github.com/foo").is_none());
    }

    #[test]
    fn parse_next_link_finds_next_url() {
        let h = reqwest::header::HeaderValue::from_static(
            "<https://api.github.com/x?page=2>; rel=\"next\", <https://api.github.com/x?page=5>; rel=\"last\"",
        );
        assert_eq!(
            parse_next_link(Some(&h)),
            Some("https://api.github.com/x?page=2".into())
        );
    }

    #[test]
    fn parse_next_link_returns_none_when_only_last() {
        let h = reqwest::header::HeaderValue::from_static(
            "<https://api.github.com/x?page=1>; rel=\"prev\", <https://api.github.com/x?page=5>; rel=\"last\"",
        );
        assert!(parse_next_link(Some(&h)).is_none());
    }

    #[test]
    fn parse_next_link_returns_none_on_missing_header() {
        assert!(parse_next_link(None).is_none());
    }
}
