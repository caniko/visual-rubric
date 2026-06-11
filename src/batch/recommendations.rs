use std::borrow::Cow;

use super::{
    AggregateStatus, AssetRubricReport, AssetRubricResult, IssueClassificationInput,
    IssueClassifier, IssueRecommendation,
};

pub(super) fn aggregate_status(assets: &[AssetRubricReport]) -> AggregateStatus {
    if assets.iter().any(|asset| {
        matches!(
            asset.result,
            AssetRubricResult::Error { .. } | AssetRubricResult::NotEvaluatedAfterError { .. }
        )
    }) {
        AggregateStatus::Error
    } else if assets
        .iter()
        .any(|asset| matches!(asset.result, AssetRubricResult::Fail { .. }))
    {
        AggregateStatus::Fail
    } else if assets
        .iter()
        .all(|asset| matches!(asset.result, AssetRubricResult::Skipped { .. }))
    {
        AggregateStatus::Skipped
    } else {
        AggregateStatus::Pass
    }
}

pub(super) fn classify_recommendations(
    classifier: Option<&dyn IssueClassifier>,
    assets: &[AssetRubricReport],
) -> Vec<IssueRecommendation> {
    let Some(classifier) = classifier else {
        return Vec::new();
    };
    let mut recommendations = Vec::with_capacity(assets.len());
    for asset in assets {
        let Some(issue_text) = issue_text(&asset.result) else {
            continue;
        };
        for recommendation in classifier.classify(IssueClassificationInput {
            asset,
            issue_text: &issue_text,
        }) {
            merge_recommendation(&mut recommendations, recommendation);
        }
    }
    recommendations.sort_by(|left, right| {
        right
            .severity
            .cmp(&left.severity)
            .then_with(|| left.id.cmp(&right.id))
    });
    recommendations
}

fn issue_text(result: &AssetRubricResult) -> Option<Cow<'_, str>> {
    match result {
        AssetRubricResult::Fail { reason, anomalies } => {
            let anomaly_len: usize = anomalies.iter().map(String::len).sum();
            let mut text = String::with_capacity(reason.len() + anomaly_len + anomalies.len());
            text.push_str(reason);
            for anomaly in anomalies {
                text.push(' ');
                text.push_str(anomaly);
            }
            Some(Cow::Owned(text))
        }
        AssetRubricResult::Error { message } => Some(Cow::Borrowed(message)),
        AssetRubricResult::NotEvaluatedAfterError {
            root_error,
            retry_hint,
        } => Some(Cow::Owned(format!("{root_error} {retry_hint}"))),
        AssetRubricResult::Pass { .. } | AssetRubricResult::Skipped { .. } => None,
    }
}

fn merge_recommendation(
    recommendations: &mut Vec<IssueRecommendation>,
    incoming: IssueRecommendation,
) {
    let existing = recommendations.iter_mut().find(|candidate| {
        candidate.id == incoming.id
            && candidate.class == incoming.class
            && candidate.suggested_fix == incoming.suggested_fix
            && candidate.candidate_modules == incoming.candidate_modules
    });
    let Some(existing) = existing else {
        recommendations.push(incoming);
        return;
    };
    existing.severity = existing.severity.max(incoming.severity);
    for asset in incoming.affected_assets {
        if !existing.affected_assets.contains(&asset) {
            existing.affected_assets.push(asset);
        }
    }
    existing.affected_assets.sort();
    for evidence in incoming.evidence {
        if !existing.evidence.contains(&evidence) && existing.evidence.len() < 5 {
            existing.evidence.push(evidence);
        }
    }
}
