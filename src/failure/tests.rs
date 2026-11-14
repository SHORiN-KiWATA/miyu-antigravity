use super::*;

#[test]
fn table() {
    let cases = [
        ("You are not logged into Antigravity", false, Class::Auth),
        ("UNAUTHENTICATED: token expired", true, Class::Auth),
        ("Please sign in again", false, Class::Auth),
        ("Quota exceeded for model gemini", false, Class::Quota),
        ("RESOURCE_EXHAUSTED", true, Class::Quota),
        ("You have reached your usage limit", false, Class::Quota),
        ("Out of credits", false, Class::Quota),
        ("Rate limit exceeded, slow down", false, Class::RateLimited),
        ("429 Too Many Requests", true, Class::RateLimited),
        ("model output error", false, Class::Retryable),
        ("model output error", true, Class::Unclassified),
        ("", false, Class::Retryable),
        // 几种都有的取靠前的：额度用完也常带「rate limit」。
        ("quota exhausted (rate limit)", false, Class::Quota),
        ("not logged in; quota unknown", false, Class::Auth),
    ];
    for (said, delivered, class) in cases {
        assert_eq!(
            classify(said, delivered),
            class,
            "{said:?} delivered={delivered}"
        );
    }
}

#[test]
fn otherwise_depends_on_whether_deltas_went_out() {
    assert_eq!(otherwise(false), Class::Retryable);
    assert_eq!(otherwise(true), Class::Unclassified);
}

#[test]
fn error_is_cut_to_500_chars() {
    let long = "错".repeat(600);
    let got = error(Class::Quota, &long);
    assert_eq!(got["class"], "quota");
    assert_eq!(got["message"].as_str().unwrap().chars().count(), 500);
    assert_eq!(error(Class::RateLimited, "x")["class"], "rate_limited");
    assert_eq!(error(Class::Retryable, "x")["class"], "retryable");
    assert_eq!(error(Class::Auth, "x")["class"], "auth");
    assert_eq!(error(Class::Unclassified, "x")["message"], "x");
}
