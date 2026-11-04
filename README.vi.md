# Claude Buddy

[English](README.md) | **Tiếng Việt**

Nhân vật nhỏ nổi trên màn hình, cho biết các phiên Claude Code đang làm gì: đang chạy, chờ bạn, đã xong hay bị lỗi. Viết
bằng Rust và Win32 thuần (không Electron, không WebView). Chỉ chạy trên Windows 10/11.

## Cài đặt

Mở PowerShell, không cần quyền admin.

```powershell
irm https://raw.githubusercontent.com/taivt0109/claude-buddy/main/install.ps1 | iex
```

Lệnh này tải app, tạo shortcut trong Start Menu, thêm hook vào Claude Code rồi mở app. `settings.json` được sao lưu trước
và các hook khác giữ nguyên. Chỉ các phiên Claude Code mở sau đó mới hiện lên.

Chạy lại lệnh này để cập nhật. Không muốn tự thêm hook thì đặt `$env:CLAUDE_BUDDY_NO_HOOKS = "1"` trước khi cài; khi đó
app sẽ hỏi lúc khởi động.

App chưa được ký số, nên Windows SmartScreen hoặc phần mềm diệt virus có thể cảnh báo.

## Gỡ

```powershell
irm https://raw.githubusercontent.com/taivt0109/claude-buddy/main/uninstall.ps1 | iex
```

Lệnh này xoá app, shortcut, mục khởi động cùng Windows và hook. Cài đặt, số liệu và nhân vật đã thêm vẫn nằm ở
`%APPDATA%\claude-buddy-rs`, trừ khi bạn đặt `$env:CLAUDE_BUDDY_PURGE = "1"` trước.

## Chạy từ mã nguồn

```
cargo build --release
target\release\claude-buddy-rs.exe
```

Cần Rust và Windows SDK (để nhúng icon). Lần đầu app hỏi có kết nối với Claude Code không. Nếu đồng ý, app thêm hook của
riêng nó vào `%USERPROFILE%\.claude\settings.json` và giữ bản sao lưu `settings.json.buddy-rs-backup-*`.

Hook lưu đường dẫn đầy đủ tới file exe. Nếu chuyển file exe đi nơi khác, hãy kết nối lại trong Cài đặt. Nếu tự xoá file
exe, hãy tắt "Kết nối Claude Code" trước (hoặc chạy `--remove-hooks`), nếu không Claude Code sẽ báo lỗi hook.

## Dùng

| Thao tác | Kết quả |
|---|---|
| Rê chuột vào nhân vật | Hiện đầy đủ trạng thái các phiên |
| Bấm nhân vật | Mở/đóng danh sách phiên (tự đóng sau khi chuột rời đi) |
| Bấm bong bóng | Mở cửa sổ của phiên cần chú ý nhất |
| Bấm một dòng phiên | Mở cửa sổ terminal/IDE của phiên đó |
| Kéo nhân vật | Tới bất kỳ đâu trên màn hình; vị trí được ghi nhớ |
| Chuột phải nhân vật hoặc icon khay | Menu nhanh |
| Bấm icon khay | Mở cửa sổ chính (Nhân vật, Mức dùng, Cài đặt) |

Đóng cửa sổ chính thì app vẫn chạy dưới khay. Thoát hẳn bằng menu chuột phải.

Giao diện có tiếng Anh và tiếng Việt, mặc định theo Windows; đổi trong Cài đặt > Ngôn ngữ.

## Không che và không làm phiền

- Bong bóng chỉ hiện vài giây khi trạng thái đổi, rộng vừa đủ nội dung. Chỉ phiên cần bạn hoặc bị lỗi mới ở lại.
- Icon khay có chấm màu theo trạng thái, nên xem nhanh được mà không cần gì trên màn hình.
- Vùng trong suốt của cửa sổ buddy cho chuột đi xuyên qua; bấm vào buddy không lấy focus khỏi app đang gõ.
- Tự ẩn khi trình chiếu, xem phim hay chơi game toàn màn hình, rồi tự hiện lại.
- Không kêu và không hiện thông báo khi đang toàn màn hình, bật Focus Assist, hoặc khi bạn đang ở đúng cửa sổ của phiên đó.
- Nhân vật có ba cỡ. Những thứ trên chỉnh trong Cài đặt.

## Vì sao nhẹ

- Mọi animation chỉ chuẩn bị một lần, trong một tiến trình con tự thoát, nên RAM dùng lúc giải mã được trả lại hết.
- Khi phát, app ánh xạ file cache thay vì nạp vào RAM, nên Windows lấy lại vùng nhớ đó bất cứ lúc nào nó cần.
- Mỗi khung hình chỉ giải nén và cập nhật đúng vùng thay đổi; khung hình không đổi thì app không cần thức dậy.
- Nhiễu video được lọc khi chuẩn bị, nên phần đứng yên của hình không phải vẽ lại.
- Animation dừng hẳn khi khoá máy, tắt màn hình, ẩn buddy hoặc có app toàn màn hình. Ảnh tĩnh không dùng timer.

## Mức dùng

Hôm nay, tuần này (từ thứ Hai), tháng này và 14 ngày gần nhất, theo giờ máy. Số liệu đếm từ transcript trong
`%USERPROFILE%\.claude\projects` (cả subagent), chỉ đọc các con số `usage`, không đọc nội dung hội thoại. Số lớn là
đầu vào + đầu ra + ghi cache; đọc cache tính riêng. Đếm tăng dần trong một tiến trình ngắn ở mức ưu tiên thấp. Không gồm
Claude dùng trên máy khác hay trên web.

## Nhân vật

Mọi nhân vật đều dùng được ngay. Thêm từ file hoặc nhập cả thư mục cùng định dạng với bản Electron gốc (`character.json`
và một file media cho mỗi trạng thái):

| Định dạng | Ghi chú |
|---|---|
| WebM VP9 có alpha | Nền trong suốt, giải mã bằng thư viện Rust kèm app |
| MP4 | Giải mã bằng Media Foundation của Windows, không có nền trong suốt |
| PNG, APNG, GIF, WebP, JPG | Ảnh nền trắng có thể tách nền khi thêm |
| SVG | Vẽ như ảnh tĩnh, hiệu ứng trong file không chạy |

Âm thanh riêng: `sounds/done` và `sounds/waiting` dạng `.wav` hoặc `.mp3`. Mỗi file chỉ giải mã một lần rồi lưu dạng nén
trong `cache`, nên phát lại rất nhẹ. Clip rất dài bị cắt bớt khi chuẩn bị (giới hạn 50 MB mỗi file).

## Cấu hình

`%APPDATA%\claude-buddy-rs\config.json`, mục `port` (mặc định 47329, khác cổng 47321 của bản Electron nên hai app chạy
song song được). Đổi cổng thì cần kết nối lại Claude Code.

## API cục bộ

Chỉ nghe trên `127.0.0.1`. Từ chối request có header `Origin` hoặc `Host` lạ, để trang web không điều khiển được app.

- `POST /hook`: dữ liệu hook thô của Claude Code
- `POST /event`: sự kiện từ công cụ khác, ví dụ `{"session_id":"a","state":"working","step":"test","label":"Backend"}`
- `GET /sessions`, `GET /usage`, `GET /health`

## Phát triển

```
cargo test
set PREVIEW_DIR=C:\some\folder && cargo test preview -- --ignored
cargo test export_app_icon -- --ignored
```

Lệnh thứ hai xuất ảnh PNG của nhân vật và cửa sổ. Lệnh thứ ba tạo lại `assets/app.ico` từ logo vẽ trong code.

Biến môi trường để kiểm tra mà không đụng dữ liệu thật: `CLAUDE_BUDDY_RS_HOME` (thư mục dữ liệu riêng, chạy song song
được), `CLAUDE_CONFIG_DIR` (thư mục Claude), `CLAUDE_BUDDY_RS_THEME=dark|light`, `CLAUDE_BUDDY_RS_BACKDROP=blur`,
`CLAUDE_BUDDY_RS_KEEP_ANIMATING=1` (không tạm dừng khi khoá máy, để đo CPU).

## Giấy phép

MIT, xem [LICENSE](LICENSE).
