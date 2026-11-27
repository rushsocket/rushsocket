use std::collections::HashMap;

use redis::{Cmd, RedisError};
use serde_json::Value;

use super::{Adapter, RedisAdapter};

pub(super) type Members = HashMap<String, Value>;
type Reply = (i64, HashMap<String, String>);

impl RedisAdapter {
    fn presence_command(&self, app: &str, channel: &str, operation: &str) -> Cmd {
        let index = format!("{}:presence_nodes:{app}:{channel}", self.prefix);
        let mut cmd = redis::cmd("EVAL");
        cmd.arg(include_str!("presence.lua"))
            .arg(2)
            .arg(&index)
            .arg(format!("{index}:{}", self.node_id))
            .arg(operation)
            .arg(20);
        cmd
    }

    async fn local_roster(&self, app: &str, channel: &str) -> String {
        let members = self.local.get_presence_members(app, channel).await;
        let encoded: HashMap<_, _> = members
            .into_iter()
            .map(|(id, info)| (id, info.to_string()))
            .collect();
        serde_json::to_string(&encoded).expect("string map is serializable")
    }

    pub(super) async fn refresh_presence(
        &self,
        app: &str,
        channels: &[String],
    ) -> Result<(), RedisError> {
        let _guard = self.presence_lock(app).lock_owned().await;
        let mut pipeline = redis::pipe();
        for channel in channels {
            if crate::protocol::channels::ChannelKind::from_name(channel).is_presence() {
                let mut cmd = self.presence_command(app, channel, "refresh");
                cmd.arg(self.local_roster(app, channel).await);
                pipeline.add_command(cmd).ignore();
            }
        }
        if !pipeline.is_empty() {
            pipeline
                .query_async::<()>(&mut self.cmd_conn.clone())
                .await?;
        }
        Ok(())
    }

    pub(super) async fn read_members(
        &self,
        app: &str,
        channel: &str,
    ) -> Result<Members, RedisError> {
        let (_, members): Reply = self
            .presence_command(app, channel, "read")
            .query_async(&mut self.cmd_conn.clone())
            .await?;
        Ok(decode(members))
    }

    pub(super) async fn join_presence(
        &self,
        app: &str,
        channel: &str,
        socket: &str,
        user: &str,
        info: &Value,
        max: usize,
    ) -> Result<(bool, Members), ()> {
        let _guard = self.presence_lock(app).lock_owned().await;
        let (status, members): Reply = self
            .presence_command(app, channel, "add")
            .arg(self.local_roster(app, channel).await)
            .arg(user)
            .arg(info.to_string())
            .arg(max)
            .query_async(&mut self.cmd_conn.clone())
            .await
            .map_err(|e| tracing::warn!("Redis presence add failed: {e}"))?;
        if status < 0 {
            return Err(());
        }
        let members = decode(members);
        let stored_info = members.get(user).unwrap_or(info);
        self.local
            .add_presence_member(app, channel, socket, user, stored_info)
            .await;
        Ok((status == 1, members))
    }

    pub(super) async fn leave_presence(
        &self,
        app: &str,
        channel: &str,
        socket: &str,
        user: &str,
    ) -> bool {
        let _guard = self.presence_lock(app).lock_owned().await;
        let last_local = self
            .local
            .remove_presence_member(app, channel, socket, user)
            .await;
        if !last_local {
            return false;
        }
        self.presence_command(app, channel, "remove")
            .arg(self.local_roster(app, channel).await)
            .arg(user)
            .query_async(&mut self.cmd_conn.clone())
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Redis presence removal failed: {e}");
                false
            })
    }
}

fn decode(members: HashMap<String, String>) -> Members {
    members
        .into_iter()
        .map(|(id, info)| (id, serde_json::from_str(&info).unwrap_or_default()))
        .collect()
}
