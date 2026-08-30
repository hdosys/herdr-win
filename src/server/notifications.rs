use crate::config;
use crate::protocol;

pub(crate) fn should_forward_toast_to_clients(delivery: config::ToastDelivery) -> bool {
    toast_notify_kind(delivery).is_some()
}

pub(crate) fn toast_notify_kind(delivery: config::ToastDelivery) -> Option<protocol::NotifyKind> {
    match delivery {
        config::ToastDelivery::Terminal => Some(protocol::NotifyKind::Toast),
        config::ToastDelivery::System => Some(protocol::NotifyKind::SystemToast),
        config::ToastDelivery::Off | config::ToastDelivery::Herdr => None,
    }
}
