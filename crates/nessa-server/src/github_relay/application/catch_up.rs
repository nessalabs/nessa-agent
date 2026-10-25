use super::{RelaySource, RelayStore};
use crate::github_relay::domain::{Binding, Failure, Page};
/// Each page commits before requesting another. A bounded pass yields to shutdown
/// and live notifications; another pass resumes from the committed cursor.
pub async fn catch_up(
    source: &dyn RelaySource,
    store: &dyn RelayStore,
    binding: &Binding,
    token: &str,
) -> Result<Page, Failure> {
    for _ in 0..128 {
        let after = store.cursor(binding).await?;
        let page = match source.page(token, binding, after).await {
            Err(Failure::Gap(through)) => {
                if through <= after || through > 9_007_199_254_740_991 {
                    return Err(Failure::InvalidResponse);
                }
                store.gap(binding, through).await?;
                return Err(Failure::Gap(through));
            }
            result => result?,
        };
        page.validate(binding, after)?;
        store.commit(binding, after, &page).await?;
        if !page.has_more {
            return Ok(page);
        }
    }
    Err(Failure::CatchUpPending)
}
