use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use nmp::{AccessContext, Demand, Filter, LiveQuery, SourceAuthority};
use nmp_native_runtime_core::PublicIdentityError;

const NETWORK_REFRESH_TIMEOUT: Duration = Duration::from_secs(3);
const CANCELLATION_POLL: Duration = Duration::from_millis(50);

pub(crate) fn public_identity_live_query(filter: Filter) -> Result<LiveQuery, PublicIdentityError> {
    // Identity reads ask the operator-configured public lanes for generic
    // public facts. A bare `from_filter` would classify this author-bearing
    // selection as AuthorOutboxes, silently excluding app relays that cache
    // canonical profile/contact events.
    Demand::new(filter, SourceAuthority::Public, AccessContext::Public)
        .map(LiveQuery)
        .map_err(|error| PublicIdentityError::Failed {
            reason: Arc::from(error.to_string()),
        })
}

pub(crate) fn receive_identity_frame(
    subscription: nmp::Subscription,
    cancellation: &nmp_native_runtime_core::Cancellation,
    closed: &AtomicBool,
    network_refresh: bool,
) -> Result<nmp::Frame, PublicIdentityError> {
    let mut frame = subscription.recv().map_err(|_| {
        if closed.load(Ordering::Acquire) {
            PublicIdentityError::Closed
        } else {
            PublicIdentityError::Failed {
                reason: Arc::from("NMP identity observation closed before its first frame"),
            }
        }
    })?;
    let refresh_deadline = Instant::now() + NETWORK_REFRESH_TIMEOUT;
    while network_refresh
        && frame
            .window
            .as_ref()
            .is_some_and(|window| window.rows.is_empty())
    {
        if cancellation.is_cancelled() {
            subscription.cancel();
            return Err(PublicIdentityError::Cancelled);
        }
        let now = Instant::now();
        if now >= refresh_deadline {
            break;
        }
        let wait = CANCELLATION_POLL.min(refresh_deadline.saturating_duration_since(now));
        match subscription.recv_timeout(wait) {
            Ok(next) => frame = next,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(if closed.load(Ordering::Acquire) {
                    PublicIdentityError::Closed
                } else {
                    PublicIdentityError::Failed {
                        reason: Arc::from("NMP identity observation closed during network refresh"),
                    }
                });
            }
        }
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeSet,
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    use nmp::{Binding, EngineConfig};
    use nmp_native_runtime_core::{
        PublicIdentityDataPlane, PublicIdentityQuery, PublicIdentityReadLimits,
    };

    use super::*;
    use crate::NmpDataPlane;

    #[test]
    fn identity_reads_use_operator_public_lanes_even_with_an_author_filter() {
        let query = public_identity_live_query(Filter {
            kinds: Some(BTreeSet::from([0])),
            authors: Some(Binding::Literal(BTreeSet::from([
                "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5".to_owned(),
            ]))),
            ..Filter::default()
        })
        .unwrap();

        assert_eq!(query.0.source, SourceAuthority::Public);
        assert_eq!(query.0.access, AccessContext::Public);
    }

    #[test]
    fn configured_identity_read_keeps_the_empty_observation_until_cancelled() {
        let plane = NmpDataPlane::open(
            EngineConfig {
                app_relays: vec!["ws://127.0.0.1:9".to_owned()],
                allowed_local_relay_hosts: vec!["127.0.0.1".to_owned()],
                ..EngineConfig::default()
            },
            2,
        )
        .unwrap();
        let frozen = plane
            .set_active_public_identity(Some(nmp_native_runtime_core::AccountRef(Arc::from(
                "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5",
            ))))
            .unwrap();
        let cancellation = nmp_native_runtime_core::Cancellation::new();
        let cancel = cancellation.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel.cancel();
        });
        let started = Instant::now();
        assert!(matches!(
            plane.read_public_identity(
                &frozen,
                PublicIdentityQuery::Profile,
                &cancellation,
                PublicIdentityReadLimits {
                    maximum_items: 8,
                    maximum_sources: 8,
                    maximum_frame_bytes: 16 * 1024,
                },
            ),
            Err(PublicIdentityError::Cancelled)
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        plane.close();
    }
}
