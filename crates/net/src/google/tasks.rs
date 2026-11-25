//! Google Tasks (API v1): the task lists, their tasks, and changes to them.

use mailalm_model::pim::{Task, TaskList};
use serde_json::{Value, json};

use super::{Api, GoogleClient, MAX_JSON, checked, convert, json_of, json_request};
use crate::http::{self, Request};
use crate::{NetError, Result};

/// Most task lists, and tasks in one list, taken.
const MAX_LISTS: usize = 500;
const MAX_TASKS: usize = 20_000;

impl GoogleClient {
    /// The task lists, the default one ("My Tasks") first, at most [`MAX_LISTS`].
    pub fn task_lists(&mut self) -> Result<Vec<TaskList>> {
        let first = format!("{}/users/@me/lists?maxResults=100", self.base(Api::Tasks));
        let items = self.pages(Api::Tasks, &first, "items", MAX_LISTS)?;
        Ok(items.iter().enumerate().filter_map(|(i, v)| convert::task_list_of(v, i == 0)).collect())
    }

    /// The tasks of a list, done and hidden ones too (deleted ones left out), at most
    /// [`MAX_TASKS`].
    pub fn tasks(&mut self, list_id: &str) -> Result<Vec<Task>> {
        let list = checked(list_id, "a task list")?;
        let first = format!("{}/lists/{}/tasks?maxResults=100&showCompleted=true&showHidden=true", self.base(Api::Tasks), http::pct(list));
        let items = self.pages(Api::Tasks, &first, "items", MAX_TASKS)?;
        Ok(items.iter().filter_map(|v| convert::task_of(v, list)).collect())
    }

    /// Make a task in a list. Not sent again once it may have been acted on.
    pub fn create_task(&mut self, list_id: &str, task: &Task) -> Result<Task> {
        let list = checked(list_id, "a task list")?.to_string();
        let url = format!("{}/lists/{}/tasks", self.base(Api::Tasks), http::pct(&list));
        let resp = self.exchange(Api::Tasks, json_request("POST", &url, &convert::task_json(task))?, true, false).map_err(|f| self.hide(f.error))?;
        self.checked(&resp)?;
        convert::task_of(&json_of(&resp)?, &list).ok_or_else(|| NetError::Protocol("Google Tasks made the task but sent it back unreadable".into()))
    }

    /// Change a task (`task.list`, `task.remote.id`): its title, notes, due day, done.
    pub fn update_task(&mut self, task: &Task) -> Result<Task> {
        let list = checked(&task.list, "a task list")?.to_string();
        let id = checked(&task.remote.id, "a task")?.to_string();
        self.patch_task(&list, &id, &convert::task_json(task))
    }

    /// Mark a task done, or not done.
    pub fn complete_task(&mut self, list_id: &str, task_id: &str, done: bool) -> Result<Task> {
        let list = checked(list_id, "a task list")?.to_string();
        let id = checked(task_id, "a task")?.to_string();
        let body = if done { json!({"status": "completed"}) } else { json!({"status": "needsAction", "completed": Value::Null}) };
        self.patch_task(&list, &id, &body)
    }

    fn patch_task(&mut self, list: &str, id: &str, body: &Value) -> Result<Task> {
        let url = format!("{}/lists/{}/tasks/{}", self.base(Api::Tasks), http::pct(list), http::pct(id));
        let resp = self.call(Api::Tasks, json_request("PATCH", &url, body)?)?;
        self.checked(&resp)?;
        convert::task_of(&json_of(&resp)?, list).ok_or_else(|| NetError::Protocol("Google Tasks changed the task but sent it back unreadable".into()))
    }

    /// Delete a task. One already gone counts as done.
    pub fn delete_task(&mut self, list_id: &str, task_id: &str) -> Result<()> {
        let list = checked(list_id, "a task list")?;
        let id = checked(task_id, "a task")?;
        let url = format!("{}/lists/{}/tasks/{}", self.base(Api::Tasks), http::pct(list), http::pct(id));
        let resp = self.call(Api::Tasks, Request::new("DELETE", &url, MAX_JSON))?;
        if matches!(resp.status, 404 | 410) {
            return Ok(());
        }
        self.checked(&resp)
    }
}
