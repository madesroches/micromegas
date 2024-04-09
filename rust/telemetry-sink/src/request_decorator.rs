pub trait RequestDecorator {
    fn decorate(&self, builder: &mut reqwest::Request);
}

pub struct TrivialRequestDecorator {}

impl RequestDecorator for TrivialRequestDecorator {
    fn decorate(&self, _request: &mut reqwest::Request) {}
}
