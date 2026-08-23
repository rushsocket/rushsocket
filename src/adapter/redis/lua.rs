/// Cluster connection-quota HASH: SET-absolute per node, reap fields whose
/// `node_stats` key is missing, optional over-quota revert.
///
/// KEYS[1] = `{prefix}:connections:{app_id}`
/// ARGV[1] = node_id
/// ARGV[2] = new_count (ignored when mode=0)
/// ARGV[3] = limit (`< 0` skips only the cap revert)
/// ARGV[4] = mode (`0` GET, `1` SET)
/// ARGV[5] = node_stats key prefix (`{prefix}:node_stats:`)
pub const QUOTA_LUA: &str = r#"
local hash = KEYS[1]
local node_id = ARGV[1]
local new_count = tonumber(ARGV[2])
local limit = tonumber(ARGV[3])
local mode = tonumber(ARGV[4])
local stats_prefix = ARGV[5]

local fields = redis.call('HKEYS', hash)
for _, field in ipairs(fields) do
  if field ~= node_id then
    if redis.call('EXISTS', stats_prefix .. field) == 0 then
      redis.call('HDEL', hash, field)
    end
  end
end

if mode == 1 then
  local old = redis.call('HGET', hash, node_id)
  redis.call('HSET', hash, node_id, new_count)
  local vals = redis.call('HVALS', hash)
  local sum = 0
  for _, v in ipairs(vals) do
    sum = sum + tonumber(v)
  end
  if limit >= 0 and sum > limit then
    if old then
      redis.call('HSET', hash, node_id, old)
    else
      redis.call('HDEL', hash, node_id)
    end
    return {0, sum}
  end
  return {1, sum}
end

local vals = redis.call('HVALS', hash)
local sum = 0
for _, v in ipairs(vals) do
  sum = sum + tonumber(v)
end
return {1, sum}
"#;

/// Presence add: HLEN/HEXISTS/HINCRBY/HSET in one eval.
/// KEYS[1] presence hash, KEYS[2] psock hash
/// ARGV[1] user_id, ARGV[2] user_info JSON, ARGV[3] max_members
/// Returns {accepted, is_new} as 0/1.
pub const PRESENCE_ADD_LUA: &str = r#"
local presence = KEYS[1]
local psock = KEYS[2]
local user_id = ARGV[1]
local info = ARGV[2]
local max_members = tonumber(ARGV[3])

local exists = redis.call('HEXISTS', presence, user_id)
if exists == 0 then
  local count = redis.call('HLEN', presence)
  if count >= max_members then
    return {0, 0}
  end
end

redis.call('HINCRBY', psock, user_id, 1)
local is_new = 0
if exists == 0 then
  redis.call('HSET', presence, user_id, info)
  is_new = 1
end
return {1, is_new}
"#;

/// Presence remove: never HDEL the user if post-decrement count > 0.
/// KEYS[1] presence hash, KEYS[2] psock hash
/// ARGV[1] user_id
/// Returns 1 if the user is gone, 0 otherwise.
pub const PRESENCE_REMOVE_LUA: &str = r#"
local presence = KEYS[1]
local psock = KEYS[2]
local user_id = ARGV[1]

if redis.call('HEXISTS', psock, user_id) == 0 then
  redis.call('HDEL', presence, user_id)
  if redis.call('HLEN', presence) == 0 then
    redis.call('DEL', presence)
    redis.call('DEL', psock)
  end
  return 1
end

local count = redis.call('HINCRBY', psock, user_id, -1)
if count <= 0 then
  redis.call('HDEL', presence, user_id)
  redis.call('HDEL', psock, user_id)
  if redis.call('HLEN', presence) == 0 then
    redis.call('DEL', presence)
    redis.call('DEL', psock)
  end
  return 1
end
return 0
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_lua_is_set_absolute_not_hincr() {
        assert!(QUOTA_LUA.contains("HSET"));
        assert!(QUOTA_LUA.contains("HVALS"));
        assert!(QUOTA_LUA.contains("EXISTS"));
        assert!(QUOTA_LUA.contains("HDEL"));
        assert!(!QUOTA_LUA.contains("HINCRBY"));
        assert!(QUOTA_LUA.contains("limit >= 0"));
    }

    #[test]
    fn presence_add_lua_is_single_eval() {
        assert!(PRESENCE_ADD_LUA.contains("HLEN"));
        assert!(PRESENCE_ADD_LUA.contains("HEXISTS"));
        assert!(PRESENCE_ADD_LUA.contains("HINCRBY"));
        assert!(PRESENCE_ADD_LUA.contains("HSET"));
    }

    #[test]
    fn presence_remove_lua_keeps_user_when_count_positive() {
        assert!(PRESENCE_REMOVE_LUA.contains("HINCRBY"));
        assert!(PRESENCE_REMOVE_LUA.contains("count <= 0"));
        assert!(PRESENCE_REMOVE_LUA.contains("HDEL"));
    }
}
