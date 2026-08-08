use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 调用方省略 `limit` 时返回的记录数量。
pub const DEFAULT_PAGE_LIMIT: u32 = 50;
/// [`PageRequest::new`] 接受的最大页面大小。
pub const MAX_PAGE_LIMIT: u32 = 200;
/// 存储层解码前 [`PageRequest::new`] 接受的最大编码游标长度。
pub const MAX_CURSOR_BYTES: usize = 512;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 单个有序结果页，以及继续同一查询的不透明游标。
pub struct CursorPage<T> {
    /// 稳定查询顺序中的记录，数量不超过请求上限。
    pub items: Vec<T>,
    /// 不透明续页令牌；不存在后续页面时为 `None`。
    pub next_cursor: Option<String>,
}

/// [`CursorPage`] 的向后兼容名称。
pub type Page<T> = CursorPage<T>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Core 存储层接受的游标分页输入。
///
/// [`Self::new`] 返回的值（包括委托给它的 HTTP 请求解析器）具有位于
/// `1..=`[`MAX_PAGE_LIMIT`] 内的 `limit`，以及长度至多为 [`MAX_CURSOR_BYTES`] 的非空游标。
/// 公开字段也允许直接构造，此时不会强制这些边界；在特定存储层解码前，游标始终不透明。
pub struct PageRequest {
    /// 前一页 [`CursorPage::next_cursor`] 提供的不透明位置。
    pub cursor: Option<String>,
    /// 要返回的最大记录数。
    pub limit: u32,
}

impl PageRequest {
    /// 构建已校验的游标请求。
    ///
    /// # Errors
    ///
    /// 当 `limit` 不在 `1..=200` 内或游标无效时返回 [`PageRequestError`]。
    pub fn new(cursor: Option<String>, limit: Option<u32>) -> Result<Self, PageRequestError> {
        if cursor
            .as_deref()
            .is_some_and(|cursor| cursor.is_empty() || cursor.len() > MAX_CURSOR_BYTES)
        {
            return Err(PageRequestError::InvalidCursor);
        }
        let limit = limit.unwrap_or(DEFAULT_PAGE_LIMIT);
        if limit == 0 || limit > MAX_PAGE_LIMIT {
            return Err(PageRequestError::InvalidLimit);
        }
        Ok(Self { cursor, limit })
    }
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
/// 分页请求抵达存储层前产生的校验失败。
pub enum PageRequestError {
    #[error("cursor must contain between 1 and 512 bytes")]
    /// 游标为空或超过 [`MAX_CURSOR_BYTES`]。
    InvalidCursor,
    #[error("page limit must be between 1 and 200")]
    /// 请求上限为零或超过 [`MAX_PAGE_LIMIT`]。
    InvalidLimit,
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PAGE_LIMIT, MAX_PAGE_LIMIT, PageRequest};

    #[test]
    fn page_request_uses_default_and_accepts_the_maximum() {
        let default = PageRequest::new(None, None).expect("default page request");
        let maximum = PageRequest::new(Some("next".to_owned()), Some(MAX_PAGE_LIMIT))
            .expect("maximum page request");

        assert_eq!(default.limit, DEFAULT_PAGE_LIMIT);
        assert_eq!(maximum.limit, MAX_PAGE_LIMIT);
        assert_eq!(maximum.cursor.as_deref(), Some("next"));
    }

    #[test]
    fn page_request_rejects_a_limit_above_the_maximum() {
        assert!(PageRequest::new(None, Some(MAX_PAGE_LIMIT + 1)).is_err());
    }

    #[test]
    fn page_request_rejects_zero_limit() {
        assert!(PageRequest::new(None, Some(0)).is_err());
    }

    #[test]
    fn page_request_rejects_an_empty_cursor() {
        assert!(PageRequest::new(Some(String::new()), None).is_err());
    }

    #[test]
    fn page_request_rejects_a_cursor_over_512_bytes_before_decoding() {
        assert!(PageRequest::new(Some("A".repeat(513)), None).is_err());
    }
}
