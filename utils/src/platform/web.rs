pub trait MaybeSend {}
impl<T> MaybeSend for T {}

pub trait MaybeSendSync: MaybeSend {}
impl<T> MaybeSendSync for T {}
