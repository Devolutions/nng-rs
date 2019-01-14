use std::os::raw::c_int;

#[derive(Debug, Copy, Clone)]
pub enum PipeEvent {
    AddPre,
    AddPost,
    RemovePost,
    Unknown(i32),
}

pub type PipeNotifyFn = FnMut(PipeEvent) + 'static;

impl PipeEvent {
    pub(crate) fn from_code(event: c_int) -> PipeEvent {
        match event {
            nng_sys::NNG_PIPE_EV_ADD_PRE => PipeEvent::AddPre,
            nng_sys::NNG_PIPE_EV_ADD_POST => PipeEvent::AddPost,
            nng_sys::NNG_PIPE_EV_REM_POST => PipeEvent::RemovePost,
            n => PipeEvent::Unknown(n),
        }
    }
}
