pub struct SchemaBuilder;

impl SchemaBuilder {
    // Types

    pub fn serial(self, name: &'static str) -> Self {
        // Implemented in macro
        self
    }

    pub fn text(self, name: &'static str) -> Self {
        // Implemented in macro
        self
    }

    pub fn int(self, name: &'static str) -> Self {
        // Implemented in macro
        self
    }

    pub fn datetime(self, name: &'static str) -> Self {
        // Implemented in macro
        self
    }

    pub fn uuid(self, name: &'static str) -> Self {
        // Implemented in macro
        self
    }

    // Modifiers

    pub fn optional(self) -> Self {
        // Implemented in macro
        self
    }

    // TODO: Make this typesafe
    pub fn default<T>(self, value: T) -> Self {
        // Implemented in macro
        self
    }
}
