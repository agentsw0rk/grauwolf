//! Active model and the stack of the models of the entered subgraphs (ADR-0043).

use grauwolf_prompt::LlModel;

/// The active model and the outer models of subgraphs with their own model.
#[derive(Clone)]
pub(crate) struct ModelSelection {
    model: LlModel,
    /// Outer models of the entered subgraphs that have their own model (ADR-0043).
    scopes: Vec<LlModel>,
}

impl ModelSelection {
    /// Selection with `model` active and no subgraph entered.
    pub(crate) fn new(model: LlModel) -> Self {
        Self {
            model,
            scopes: Vec::new(),
        }
    }

    /// Active model.
    pub(crate) fn model(&self) -> &LlModel {
        &self.model
    }

    /// The model outside all subgraphs: what a checkpoint stores, because when resuming the
    /// scopes entered again set their models anew (ADR-0043).
    pub(crate) fn outermost(&self) -> &LlModel {
        self.scopes.first().unwrap_or(&self.model)
    }

    /// Switches the model and returns the previous one.
    pub(crate) fn set_model(&mut self, model: LlModel) -> LlModel {
        std::mem::replace(&mut self.model, model)
    }

    /// The active model, given up.
    pub(crate) fn into_model(self) -> LlModel {
        self.model
    }

    /// Enters a subgraph with its own model; the previous one is remembered.
    pub(crate) fn enter_scope(&mut self, model: LlModel) {
        let outer = self.set_model(model);
        self.scopes.push(outer);
    }

    /// Leaves the innermost subgraph with its own model and restores the outer one.
    pub(crate) fn leave_scope(&mut self) {
        if let Some(outer) = self.scopes.pop() {
            self.model = outer;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str) -> LlModel {
        LlModel::new("test", id)
    }

    #[test]
    fn set_model_returns_the_previous_model() {
        let mut selection = ModelSelection::new(model("a"));
        let previous = selection.set_model(model("b"));
        assert_eq!(previous, model("a"));
        assert_eq!(selection.model(), &model("b"));
    }

    #[test]
    fn scopes_restore_the_outer_models_in_order() {
        let mut selection = ModelSelection::new(model("a"));
        selection.enter_scope(model("b"));
        selection.enter_scope(model("c"));
        assert_eq!(selection.model(), &model("c"));
        selection.leave_scope();
        assert_eq!(selection.model(), &model("b"));
        selection.leave_scope();
        assert_eq!(selection.model(), &model("a"));
    }

    #[test]
    fn leaving_without_a_scope_keeps_the_model() {
        let mut selection = ModelSelection::new(model("a"));
        selection.leave_scope();
        assert_eq!(selection.model(), &model("a"));
    }

    #[test]
    fn outermost_is_the_model_outside_all_scopes() {
        let mut selection = ModelSelection::new(model("a"));
        assert_eq!(selection.outermost(), &model("a"));
        selection.enter_scope(model("b"));
        selection.enter_scope(model("c"));
        assert_eq!(selection.outermost(), &model("a"));
    }

    #[test]
    fn set_model_inside_a_scope_is_undone_when_leaving() {
        let mut selection = ModelSelection::new(model("a"));
        selection.enter_scope(model("b"));
        selection.set_model(model("c"));
        selection.leave_scope();
        assert_eq!(selection.model(), &model("a"));
    }

    #[test]
    fn clone_is_independent() {
        let mut selection = ModelSelection::new(model("a"));
        selection.enter_scope(model("b"));
        let mut fork = selection.clone();
        fork.leave_scope();
        assert_eq!(fork.model(), &model("a"));
        assert_eq!(selection.model(), &model("b"));
    }
}
