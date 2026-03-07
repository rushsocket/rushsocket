use std::collections::HashMap;
use std::sync::Arc;

use super::{App, AppManager};
use crate::config::AppConfig;

pub struct StaticAppManager {
    by_key: HashMap<String, Arc<App>>,
    by_id: HashMap<String, Arc<App>>,
}

impl StaticAppManager {
    pub fn new(apps: Vec<AppConfig>) -> Self {
        let mut by_key = HashMap::new();
        let mut by_id = HashMap::new();
        for cfg in apps {
            let app = Arc::new(App::from(cfg));
            by_key.insert(app.key.clone(), app.clone());
            by_id.insert(app.id.clone(), app);
        }
        Self { by_key, by_id }
    }
}

impl AppManager for StaticAppManager {
    fn find_by_key(&self, key: &str) -> Option<Arc<App>> {
        self.by_key.get(key).cloned()
    }

    fn find_by_id(&self, id: &str) -> Option<Arc<App>> {
        self.by_id.get(id).cloned()
    }
}
