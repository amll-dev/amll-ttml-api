# AMLL TTML API 接口文档

**基础信息**

* **协议**: HTTP/HTTPS
* **内容类型**: `application/json`
* **CORS**: 允许跨域 (`Access-Control-Allow-Origin: *`)

---

## 1. 数据模型

### 1.1 歌曲元数据与歌词模型

所有的成功响应均返回此模型构成的 JSON 数组。

| 字段名            | 类型               | 说明                                                |
| ----------------- | ------------------ | --------------------------------------------------- |
| `filename`        | `string`           | 歌词文件名                                          |
| `trackNames`      | `string[]`         | 歌曲名列表                                          |
| `artistNames`     | `string[]`         | 歌手名列表                                          |
| `albumNames`      | `string[]`         | 专辑名列表                                          |
| `ncmMusicIds`     | `string[]`         | 网易云音乐平台 ID 列表                              |
| `qqMusicIds`      | `string[]`         | QQ 音乐平台 ID 列表                                 |
| `appleMusicIds`   | `string[]`         | Apple Music 平台 ID 列表                            |
| `spotifyIds`      | `string[]`         | Spotify 平台 ID 列表                                |
| `isrcs`           | `string[]`         | ISRC 国际标准音像制品编码列表                       |
| `authorIds`       | `string[]`         | TTML 歌词贡献者的 GitHub ID 列表                    |
| `authorUsernames` | `string[]`         | TTML 歌词贡献者的 GitHub 用户名列表                 |
| `syncedLyrics`    | `string` \| `null` | TTML 格式的歌词。在 `/api/search` 中始终为 `null`。 |
| `plainLyrics`     | `string` \| `null` | 纯文本歌词（预留字段，当前为 `null`）               |

### 1.2 错误响应模型

| 字段名    | 类型     | 说明                        |
| --------- | -------- | --------------------------- |
| `status`  | `number` | HTTP 状态码                 |
| `error`   | `string` | 错误简述 (如 "Bad Request") |
| `message` | `string` | 详细的错误原因说明          |

---

## 2. 接口列表

### 2.1 获取歌词

通过唯一的 ID 获取一首歌曲的元数据及其完整的 TTML 歌词。

* **路径**: `/api/get`
* **方法**: `GET`

#### 请求参数

至少需要提供以下任意一个参数。如果提供多个不同平台的参数，或为同一平台提供多个 ID，将使用交集 (AND) 进行匹配。

如果未提供任何有效参数，将返回 `400 Bad Request`。如果获取歌词失败，将返回 `502 Bad Gateway`。

| 参数名         | 类型     | 必填 | 说明           |
| -------------- | -------- | ---- | -------------- |
| `ncmMusicId`   | `string` | 否   | 网易云音乐 ID  |
| `qqMusicId`    | `string` | 否   | QQ 音乐 ID     |
| `appleMusicId` | `string` | 否   | Apple Music ID |
| `spotifyId`    | `string` | 否   | Spotify ID     |
| `isrc`         | `string` | 否   | ISRC 编码      |

#### 响应示例

**成功 (200 OK)** - 返回长度为 0 或 1 的数组：

```json
[
  {
    "filename": "1768754400682-250306205-r6IrpmBd.ttml",
    "trackNames": [
      "ME!",
      "ME! (feat. Brendon Urie of Panic! At The Disco)"
    ],
    "artistNames": [
      "Brendon Urie",
      "Taylor Swift"
    ],
    "albumNames": [
      "Lover"
    ],
    "ncmMusicIds": [
      "1361348080",
      "1382781549"
    ],
    "qqMusicIds": [
      "0032UZe62rZk9K"
    ],
    "appleMusicIds": [
      "1468058706"
    ],
    "spotifyIds": [
      "2Rk4JlNc2TPmZe2af99d45"
    ],
    "isrcs": [
      "USUG11901494"
    ],
    "authorIds": [
      "108002475",
      "132769718",
      "207428447",
      "250306205",
      "34237075",
      "50747104"
    ],
    "authorUsernames": [
      "SteamFinder",
      "Xionghaizi001",
      "Y-CIAO",
      "apoint123",
      "kid1412520",
      "kid141252010"
    ],
    "syncedLyrics": "ttml字符串...",
    "plainLyrics": null
  }
]
```

---

### 2.2 搜索歌词

在词库中搜索符合条件的歌词。

* **路径**: `/api/search`
* **方法**: `GET`

#### 请求参数

至少需要提供以下任意一个参数。如果提供多个参数，将使用交集 (AND) 匹配。

| 参数名                  | 别名 / 兼容参数                                                               | 类型     | 必填 | 匹配逻辑 | 说明                                   |
| ----------------------- | ----------------------------------------------------------------------------- | -------- | ---- | -------- | -------------------------------------- |
| `q`                     | -                                                                             | `string` | 否   | 模糊包含 | 全局搜索，命中曲名、歌手、专辑其一即可 |
| `musicNames`            | `trackNames` / `trackName` / `track_name` / `track_names`                     | `string` | 否   | 模糊包含 | 限定匹配曲名                           |
| `artistNames`           | `artists` / `artistName` / `artist_name` / `artist_names`                     | `string` | 否   | 模糊包含 | 限定匹配歌手名                         |
| `albumNames`            | `album` / `albumName` / `album_name` / `album_names`                          | `string` | 否   | 模糊包含 | 限定匹配专辑名                         |
| `ttmlAuthorGithub`      | `authorIds` / `authorId` / `author_id` / `author_ids`                         | `string` | 否   | 严格全等 | TTML 贡献者的 GitHub ID                |
| `ttmlAuthorGithubLogin` | `authorUserNames` / `authorUserName` / `author_username` / `author_usernames` | `string` | 否   | 严格全等 | TTML 贡献者的 GitHub 用户名            |

#### 注意事项

1. 如果未提供任何合法参数，将返回 `400 Bad Request`。
2. 文本字段采用忽略大小写的 ASCII 包含匹配，作者字段采用严格全等匹配。
3. 结果按照时间戳降序排序。
4. 最多返回前 **50** 条记录。
5. `syncedLyrics` 始终为 `null`。你需要使用 `/api/get` 来获取歌词内容。

#### 响应示例

```json
[
  {
    "filename": "1768754400682-250306205-r6IrpmBd.ttml",
    "trackNames": [
      "ME!",
      "ME! (feat. Brendon Urie of Panic! At The Disco)"
    ],
    "artistNames": [
      "Brendon Urie",
      "Taylor Swift"
    ],
    "albumNames": [
      "Lover"
    ],
    "ncmMusicIds": [
      "1361348080",
      "1382781549"
    ],
    "qqMusicIds": [
      "0032UZe62rZk9K"
    ],
    "appleMusicIds": [
      "1468058706"
    ],
    "spotifyIds": [
      "2Rk4JlNc2TPmZe2af99d45"
    ],
    "isrcs": [
      "USUG11901494"
    ],
    "authorIds": [
      "108002475",
      "132769718",
      "207428447",
      "250306205",
      "34237075",
      "50747104"
    ],
    "authorUsernames": [
      "SteamFinder",
      "Xionghaizi001",
      "Y-CIAO",
      "apoint123",
      "kid1412520",
      "kid141252010"
    ],
    "syncedLyrics": null,
    "plainLyrics": null
  },
  {
    "filename": "1743791094539-108002475-d974b0fc.ttml",
    "trackNames": [
      "ME! (feat. Brendon Urie of Panic! At The Disco)"
    ]
    ...
  }
]
```

---

## 3. 错误代码说明

| HTTP 状态码 | 业务场景                                                        | 处理建议                                                            |
| ----------- | --------------------------------------------------------------- | ------------------------------------------------------------------- |
| `400`       | 参数验证失败（未传入任何有效请求参数）                          | 请检查客户端构造的 URL query 参数是否正确且包含至少一个有效搜索项。 |
| `404`       | 路由不存在                                                      | 请检查请求的 API 路径是否正确。                                     |
| `500`       | 内部服务器错误或 JSON 序列化异常                                | 通常为内存或计算异常，需联系接口维护者。                            |
| `502`       | 网关错误（未能从 GitHub 获取到 TTML 原文 / 索引数据库更新失败） | 远端数据源不稳定，建议客户端实现重试机制或稍后访问。                |
