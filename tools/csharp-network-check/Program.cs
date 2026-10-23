using XCode.Services;

// 用 DH.NCode 自带的 NetworkDb 客户端（DbClient），验证 provider=network 真能连 Pek.RDbProxy 并执行 SQL。
//
// 用法（代理需先启动，数据源与令牌按实际替换）：
//   dotnet run -c Release -- http://127.0.0.1:5599 demo tk-demo-change-me
//
// 连接串格式与面板给出的一致：
//   Server=http://host:port;Database=<数据源名>;Password=<令牌>;provider=network

var server = args.Length > 0 ? args[0] : "http://127.0.0.1:5599";
var db = args.Length > 1 ? args[1] : "demo";
var token = args.Length > 2 ? args[2] : "tk-demo-change-me";

Console.WriteLine($"[Config] {server} db={db} token={(token.Length > 4 ? token[..4] + "****" : "****")}");

var client = new DbClient(server, db, token);

var info = await client.LoginAsync();
Console.WriteLine($"[Login] DbType={info?.DbType} Version={info?.Version}");

var table = await client.QueryAsync("SELECT 1 AS id, 'hello' AS name");
Console.WriteLine($"[Query] rows={table?.Rows.Count}");
if (table != null && table.Rows.Count > 0)
{
    var row = table.Rows[0];
    Console.WriteLine($"[Query] col0={row[0]} col1={row[1]}");
}

var count = await client.QueryCountAsync("sqlite_master");
Console.WriteLine($"[Count] sqlite_master={count}");

Console.WriteLine("[OK] provider=network 可用");
