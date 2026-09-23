/// The kind of output a task asks for, which decides the models that can do it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCategory {
    Text,
    Image,
    Voice,
    Music,
    Model3D,
    Video,
}
