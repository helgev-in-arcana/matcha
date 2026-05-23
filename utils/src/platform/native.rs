pub trait MaybeSend: Send {}
impl<T: Send> MaybeSend for T {}

pub trait MaybeSendSync: Send + Sync + MaybeSend {}
impl<T: Send + Sync> MaybeSendSync for T {}
