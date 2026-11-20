//! Several tools from an `impl` block with `#[toolset]` (F03.5, ADR-0003).

use std::sync::{Arc, Mutex};

use futures_executor::block_on;
use grauwolf_macros::toolset;
use grauwolf_tools::{DynTool, ToolError, ToolRegistry};
use serde_json::json;

/// A small shop with a shopping cart; all tools share it.
struct Shop {
    cart: Mutex<Vec<String>>,
}

#[toolset]
impl Shop {
    /// Adds an item to the shopping cart.
    #[tool]
    fn add_to_cart(&self, #[arg("Name of the item")] item: String) -> Result<String, ToolError> {
        self.cart.lock().unwrap().push(item);
        Ok(self.summary())
    }

    /// Counts the items in the shopping cart.
    #[tool]
    async fn count_items(&self) -> Result<usize, ToolError> {
        Ok(self.cart.lock().unwrap().len())
    }

    /// Not a tool method: stays an ordinary method.
    fn summary(&self) -> String {
        self.cart.lock().unwrap().join(", ")
    }
}

#[test]
fn turns_the_tool_methods_of_an_impl_block_into_tools_sharing_their_object() {
    let shop = Arc::new(Shop {
        cart: Mutex::new(Vec::new()),
    });
    let tools = Arc::clone(&shop).tools();
    let names: Vec<String> = tools.iter().map(|tool| tool.name().to_owned()).collect();
    let registry = ToolRegistry::default().extended(tools);

    let added = block_on(registry.call("add_to_cart", json!({ "item": "Bread" }))).unwrap();
    let count = block_on(registry.call("count_items", json!({}))).unwrap();

    assert_eq!(names, ["add_to_cart", "count_items"]);
    assert_eq!(added, "\"Bread\"");
    assert_eq!(count, "1");
    assert_eq!(shop.summary(), "Bread");
    let descriptor = ShopAddToCart(Arc::clone(&shop)).descriptor();
    assert_eq!(descriptor.description, "Adds an item to the shopping cart.");
    assert_eq!(
        descriptor.parameters["properties"]["item"]["description"],
        json!("Name of the item")
    );
}
