//! 活着的 agy 对话的登记表（设计第四节第 1、2 条，第八节「清理」）：怎么认一次 `driver.call` 是哪段对话的下一轮，表里每
//! 一项的状态怎么变。纯逻辑，不碰进程和文件：每一项带着的活进程是类型参数 `L`，起、收由调用的人（[`crate::live`]）做。
//!
//! 一项是闲着（`L` 在表里）或者正在一次 `driver.call` 里（`L` 被那一次拿走了）。拿走的 [`Table::claim`]；用完了的成功交回
//! [`Table::release`]、出了错的 [`Table::forget`]。新开的说完了才登记（[`Table::insert`]）。表里超过上限时挤掉最久没用的
//! 闲着的；闲置到点的、进程自己退了的由巡检拿掉（[`Table::sweep`]）。

use std::collections::HashSet;
use std::time::{Duration, Instant};

/// 表里至多几项（闲着的加正忙的）。
pub const LIMIT: usize = 4;

/// 除了消息以外要一样的：模型、system 的指纹、工具面的指纹。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `params.model`。
    pub model: String,
    /// `fingerprint.system`。
    pub system: String,
    /// `fingerprint.tools`。
    pub tools: String,
}

/// 进来的一次 `driver.call`：认对话要的几样。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incoming {
    /// 模型和两个指纹。
    pub identity: Identity,
    /// `fingerprint.messages`：一条消息一个。
    pub messages: Vec<String>,
    /// `request.messages` 每条的 `role`，和 `messages` 一样长。
    pub roles: Vec<String>,
}

/// 登记着的一段对话记的：认它要的，和代理名、上次用完的时刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// 模型和两个指纹。
    pub identity: Identity,
    /// 上一次请求的 `fingerprint.messages`。
    pub messages: Vec<String>,
    /// 代理名（system 的哈希）。
    pub agent: String,
    /// 上次用完的时刻。
    pub used: Instant,
}

/// `incoming` 是不是 `meta` 那段对话的新的一轮：模型、两个指纹都一样；记着的那串（m 条）是这一次的（n 条）的严格前缀；
/// 第 m 条以后依次是恰好一条 `assistant`、一条或几条 `user`。是的交 m。
#[must_use]
pub fn next_turn(meta: &Meta, incoming: &Incoming) -> Option<usize> {
    let known = meta.messages.len();
    let extra = incoming.roles.get(known..)?;
    let shaped = extra.first().is_some_and(|role| role == "assistant")
        && extra.len() >= 2
        && extra[1..].iter().all(|role| role == "user");
    let matches = meta.identity == incoming.identity
        && incoming.messages.len() == incoming.roles.len()
        && incoming.messages.starts_with(&meta.messages);
    (shaped && matches).then_some(known)
}

/// 一项：编号、记的、闲着时的活进程（正忙的是 `None`）。
#[derive(Debug)]
struct Slot<L> {
    id: u64,
    meta: Meta,
    live: Option<L>,
}

/// [`Table::claim`] 拿到的：哪一项、它的活进程、记着的那串有几条（这一次从第几条起是新的）。
#[derive(Debug)]
pub struct Claimed<L> {
    /// 表里的编号，交回、忘掉时用。
    pub id: u64,
    /// 活进程。
    pub live: L,
    /// 记着的那串的长度 m：这一次的第 m 条以后是新的。
    pub known: usize,
    /// 代理名。
    pub agent: String,
}

/// 登记表。
#[derive(Debug)]
pub struct Table<L> {
    next_id: u64,
    limit: usize,
    slots: Vec<Slot<L>>,
}

impl<L> Table<L> {
    /// 空表，至多 `limit` 项。
    #[must_use]
    pub fn new(limit: usize) -> Table<L> {
        Table {
            next_id: 0,
            limit,
            slots: Vec::new(),
        }
    }

    /// 几项（闲着的加正忙的）。
    #[cfg(test)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// 表是空的。
    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 找 `incoming` 接得上的闲着的一项（[`next_turn`]）：几个都接得上的取记着的最长的（一样长又都接得上的就是同一段，
    /// [`Table::insert`] 不会留两份）。找到了把它改成正忙、交出活进程。
    pub fn claim(&mut self, incoming: &Incoming) -> Option<Claimed<L>> {
        let index = self
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.live.is_some())
            .filter_map(|(index, slot)| Some((index, next_turn(&slot.meta, incoming)?)))
            .max_by_key(|(_, known)| *known)
            .map(|(index, _)| index)?;
        let slot = &mut self.slots[index];
        let live = slot.live.take()?;
        Some(Claimed {
            id: slot.id,
            live,
            known: slot.meta.messages.len(),
            agent: slot.meta.agent.clone(),
        })
    }

    /// 正忙的 `id` 这一轮成功了：记下这一次的 `messages`、改回闲着、记时刻 `now`。交被挤掉的（见 [`Table::insert`]）。
    /// 不在表里的（例如已经被收掉了）原样交回 `live`，由调用的人收。
    pub fn release(&mut self, id: u64, live: L, messages: Vec<String>, now: Instant) -> Vec<L> {
        let Some(index) = self.slots.iter().position(|slot| slot.id == id) else {
            return vec![live];
        };
        let slot = self.slots.remove(index);
        let meta = Meta {
            messages,
            used: now,
            ..slot.meta
        };
        self.insert(meta, live)
    }

    /// 正忙的 `id` 出了错、进程已经收了：从表里拿掉。
    pub fn forget(&mut self, id: u64) {
        self.slots.retain(|slot| slot.id != id);
    }

    /// 登记一段闲着的对话。同一段（模型、指纹、消息一样）的旧的闲着的换掉；超过上限的挤掉最久没用的闲着的（不挤刚登记的
    /// 这一项）。交拿掉的活进程，由调用的人收。
    pub fn insert(&mut self, meta: Meta, live: L) -> Vec<L> {
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.slots.len() {
            let slot = &self.slots[index];
            if slot.live.is_some()
                && slot.meta.identity == meta.identity
                && slot.meta.messages == meta.messages
            {
                removed.extend(self.slots.remove(index).live);
            } else {
                index += 1;
            }
        }
        self.slots.push(Slot {
            id: self.next_id,
            meta,
            live: Some(live),
        });
        self.next_id += 1;
        // 刚登记的这一项（在最后）不挤：别的都正忙着时照样留下，超过上限也不收正忙的。
        while self.slots.len() > self.limit {
            let newest = self.slots.len() - 1;
            let oldest = self.slots[..newest]
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.live.is_some())
                .min_by_key(|(_, slot)| slot.meta.used)
                .map(|(index, _)| index);
            let Some(oldest) = oldest else { break };
            removed.extend(self.slots.remove(oldest).live);
        }
        removed
    }

    /// 巡检：拿掉闲着的、闲置超过 `keep` 的（以 `now` 算）和 `dead` 说进程没了的，交它们的活进程。
    pub fn sweep(&mut self, now: Instant, keep: Duration, dead: impl Fn(&L) -> bool) -> Vec<L> {
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.slots.len() {
            let slot = &self.slots[index];
            let expired = now.saturating_duration_since(slot.meta.used) >= keep;
            if slot.live.as_ref().is_some_and(|live| expired || dead(live)) {
                removed.extend(self.slots.remove(index).live);
            } else {
                index += 1;
            }
        }
        removed
    }

    /// 扩展要退出：拿掉所有闲着的，交它们的活进程（正忙的由它们那一次收）。
    pub fn drain(&mut self) -> Vec<L> {
        let mut removed = Vec::new();
        self.slots.retain_mut(|slot| match slot.live.take() {
            Some(live) => {
                removed.push(live);
                false
            }
            None => true,
        });
        removed
    }

    /// 表里各项用着的代理名。
    #[must_use]
    pub fn agents(&self) -> HashSet<String> {
        self.slots
            .iter()
            .map(|slot| slot.meta.agent.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests;
