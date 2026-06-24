# AMLL TTML API 接口文档

**基础信息**

* **协议**: HTTP/HTTPS
* **内容类型**: `application/json`
* **CORS**: 允许跨域 (`Access-Control-Allow-Origin: *`)

---

## 1. 数据模型

### 1.1 歌曲元数据与歌词模型

| 字段名            | 类型               | 说明                                                                |
| ----------------- | ------------------ | ------------------------------------------------------------------- |
| `filename`        | `string`           | 歌词文件名                                                          |
| `musicNames`      | `string[]`         | 歌曲名列表                                                          |
| `artistNames`     | `string[]`         | 歌手名列表                                                          |
| `albumNames`      | `string[]`         | 专辑名列表                                                          |
| `ncmMusicIds`     | `string[]`         | 网易云音乐平台 ID 列表                                              |
| `qqMusicIds`      | `string[]`         | QQ 音乐平台 ID 列表                                                 |
| `appleMusicIds`   | `string[]`         | Apple Music 平台 ID 列表                                            |
| `spotifyIds`      | `string[]`         | Spotify 平台 ID 列表                                                |
| `isrcs`           | `string[]`         | ISRC 国际标准音像制品编码列表                                       |
| `authorIds`       | `string[]`         | TTML 歌词贡献者的 GitHub ID 列表                                    |
| `authorUsernames` | `string[]`         | TTML 歌词贡献者的 GitHub 用户名列表                                 |
| `lyrics`          | `string` \| `null` | TTML 格式的歌词内容。仅在 `get` 接口中返回，`search` 接口中不包含。 |
| `format`          | `string` \| `null` | 歌词格式标识。仅在获取接口中返回，当前固定为 `ttml`。保留字段。     |

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

* **路径**: `/api/v1/lyrics/get`
* **方法**: `GET`

#### 请求参数

至少需要提供以下任意一个 ID 参数。如果提供多个不同平台的参数，或为同一平台提供多个 ID，将使用交集 (AND) 进行匹配。返回匹配结果中最新的一条。

| 参数名         | 类型       | 必填 | 说明                                                               |
| -------------- | ---------- | ---- | ------------------------------------------------------------------ |
| `ncmMusicId`   | `string[]` | 否   | 网易云音乐 ID                                                      |
| `qqMusicId`    | `string[]` | 否   | QQ 音乐 ID                                                         |
| `appleMusicId` | `string[]` | 否   | Apple Music ID                                                     |
| `spotifyId`    | `string[]` | 否   | Spotify ID                                                         |
| `isrc`         | `string[]` | 否   | ISRC 编码                                                          |
| `format`       | `string`   | 否   | 保留字段，默认为 `ttml`。当前仅支持 `ttml`，传入其他值将返回 400。 |

#### 响应示例

**成功 (200 OK)**

```json
{
  "status": 200,
  "data": {
    "filename": "1768754400682-250306205-r6IrpmBd.ttml",
    "musicNames": [
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
    "lyrics": "ttml字符串...",
    "format": "ttml"
  }
}
```

**未找到 (404 Not Found)**

```json
{
  "status": 404,
  "error": "Not Found",
  "message": "No lyrics found for the provided query."
}
```

---

### 2.2 搜索歌词

在词库中搜索符合条件的歌词。为了保证接口性能，搜索结果中不包含完整歌词，只返回基础信息。

* **路径**: `/api/v1/lyrics/search`
* **方法**: `GET`

#### 请求参数

至少需要提供以下任意一个参数。可以组合多个参数以提高精确度。如果同时传入 `q` 和其他参数，则忽略 `q`，优先使用其他参数。多个非 `q` 参数之间为 AND 交集关系。空字符串参数视为未传入。

| 参数名           | 类型     | 必填 | 匹配逻辑 | 说明                                       |
| ---------------- | -------- | ---- | -------- | ------------------------------------------ |
| `q`              | `string` | 否   | 模糊包含 | 模糊搜索，可组合歌曲名、歌手名和专辑名传入 |
| `musicName`      | `string` | 否   | 模糊包含 | 限定匹配曲名                               |
| `artistName`     | `string` | 否   | 模糊包含 | 限定匹配歌手名                             |
| `albumName`      | `string` | 否   | 模糊包含 | 限定匹配专辑名                             |
| `authorId`       | `string` | 否   | 严格全等 | TTML 贡献者的 GitHub ID                    |
| `authorUsername` | `string` | 否   | 严格全等 | TTML 贡献者的 GitHub 用户名                |

#### 注意事项

1. 建议使用 `musicName`、`artistName` 或 `albumName` 而不是 `q` 进行搜索以提高精确度。
2. 文本字段采用忽略大小写的 ASCII 包含匹配，作者字段采用严格全等匹配。
3. 结果按匹配相关性降序排序，相关性相同时按时间戳降序排序。
4. 最多返回前 **50** 条记录。

#### 响应示例

**成功 (200 OK)**

```json
{
  "status": 200,
  "data": {
    "items": [
      {
        "filename": "1768754400682-250306205-r6IrpmBd.ttml",
        "musicNames": [
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
        ]
      },
      {
        // 其他搜索结果...
      }
    ]
  }
}
```

---

## 3. 错误代码说明

| HTTP 状态码 | 业务场景                                                        | 处理建议                                                            |
| ----------- | --------------------------------------------------------------- | ------------------------------------------------------------------- |
| `400`       | 参数验证失败（未传入任何有效请求参数、不支持的 format 值等）    | 请检查客户端构造的 URL query 参数是否正确且包含至少一个有效搜索项。 |
| `404`       | 路由不存在，或获取接口未匹配到任何歌词                          | 请检查请求的 API 路径以及传入的 ID 是否正确。                       |
| `500`       | 内部服务器错误或 JSON 序列化异常                                | 通常为内存或计算异常，需联系接口维护者。                            |
| `502`       | 网关错误（未能从 GitHub 获取到 TTML 原文 / 索引数据库更新失败） | 远端数据源不稳定，建议客户端实现重试机制或稍后访问。                |
