use uuid::Uuid;

/// 生成 `tk_<14 字符>` 形式的 task_id。
pub fn new_task_id() -> String {
    let u = Uuid::new_v4().simple().to_string();
    format!("tk_{}", &u[..14])
}
