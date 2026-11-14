//! Опрос USB HID. Устройства поднимает перечисление xHCI.

pub fn poll() {
    super::usb_msc::poll_hid();
}
