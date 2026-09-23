/// Output stream type
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum OutputKind {
    Stdout,
    Stderr,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_kind_clone_copy() {
        let kind = OutputKind::Stdout;
        let cloned = kind;
        let copied = kind;

        assert!(matches!(cloned, OutputKind::Stdout));
        assert!(matches!(copied, OutputKind::Stdout));

        let kind = OutputKind::Stderr;
        assert!(matches!(kind, OutputKind::Stderr));
    }

    #[test]
    fn test_output_kind_debug() {
        let kind = OutputKind::Stdout;
        let debug_str = format!("{:?}", kind);
        assert!(debug_str.contains("Stdout"));
    }
}
