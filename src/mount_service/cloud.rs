// Adding a cloud drive through the mount service: the person picks a
// service, the sign-in runs in their browser, and the finished drive joins
// the saved network locations.

use super::{CloudProvider, ServiceError, call};

// The Add a Cloud Drive dialog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CloudState {
    // Picking a service; `error` is why the last attempt did not start.
    Choose {
        error: Option<String>,
    },
    // The Nextcloud server address, asked before signing in.
    Server {
        server: String,
        error: Option<String>,
    },
    // Asking the service to start the sign-in; `server` is kept for a retry.
    Starting {
        provider: CloudProvider,
        server: String,
    },
    // The browser sign-in for the cloud item `id` (a storage id) is open.
    // `service` names the cloud service. `listed` is set once the inventory
    // shows the item, so its later disappearance means the sign-in ended
    // without a drive.
    SigningIn {
        id: String,
        service: String,
        listed: bool,
    },
    // The drive was added.
    Done {
        id: String,
        name: String,
    },
    // The sign-in for `service` ended without a drive.
    Ended {
        service: String,
    },
}

impl CloudState {
    pub(crate) fn new() -> Self {
        Self::Choose { error: None }
    }

    // How the dialog follows the cloud item `id` in a new inventory.
    pub(crate) fn follow(&mut self, locations: &[crate::storage::Location]) {
        let Self::SigningIn {
            id,
            service,
            listed,
        } = self
        else {
            return;
        };
        match locations.iter().find(|location| location.id == *id) {
            Some(location)
                if location
                    .service
                    .as_ref()
                    .is_some_and(|service| service.signing_in) =>
            {
                *listed = true;
            }
            Some(location) => {
                *self = Self::Done {
                    id: location.id.clone(),
                    name: location.name.clone(),
                };
            }
            None if *listed => {
                *self = Self::Ended {
                    service: std::mem::take(service),
                };
            }
            None => {}
        }
    }
}

// Starts adding a cloud drive and returns the service id of its item, which
// shows `signing_in` while the browser sign-in is open. `server` is the
// Nextcloud address and empty for the other services.
pub(crate) async fn add_cloud(
    provider: CloudProvider,
    server: String,
) -> Result<String, ServiceError> {
    call::<_, String>("AddCloud", &(provider.key(), server.trim())).await
}
