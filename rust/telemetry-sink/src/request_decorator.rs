pub trait RequestDecorator {
    fn decorate(&mut self, builder: &mut reqwest::Request);
}

pub struct TrivialRequestDecorator {}

impl RequestDecorator for TrivialRequestDecorator {
    fn decorate(&mut self, _request: &mut reqwest::Request) {}
}
