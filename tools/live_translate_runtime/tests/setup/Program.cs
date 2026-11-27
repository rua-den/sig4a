using System.Buffers.Binary;
using System.Diagnostics;
using System.Formats.Tar;
using System.Net;
using System.Security.Cryptography;
using System.Text;
using ICSharpCode.SharpZipLib.BZip2;

static void Check(bool accepted)
{
    if (!accepted) throw new InvalidOperationException("Unexpected English setup fixture result.");
}
static async Task Fails(Func<Task> work, string code)
{
    try { await work(); }
    catch (Exception error) when (EnglishModelInstaller.ErrorCode(error) == code) { return; }
    throw new InvalidOperationException("Missing bounded setup refusal: " + code);
}
static async Task Blocks(Action work)
{
    try { work(); }
    catch (Exception e) when (e is IOException or UnauthorizedAccessException) { return; }
    await Task.Yield();
    throw new InvalidOperationException("Competing fixture unexpectedly mutated a guarded model object.");
}
static string Sha(byte[] raw) => Convert.ToHexString(SHA256.HashData(raw));
static (EnglishModelPins.Manifest Pins, byte[] Archive) Fixture(bool symlink = false)
{
    const string id = "test-pinned-model";
    using var tar = new MemoryStream();
    var names = EnglishModelPins.Pins.Members.Keys;
    var members = new Dictionary<string,EnglishModelPins.Member>(StringComparer.Ordinal);
    using (var writer = new TarWriter(tar, TarEntryFormat.Ustar, true))
    {
        writer.WriteEntry(new UstarTarEntry(TarEntryType.Directory, id + "/"));
        foreach (var name in names)
        {
            var bytes = Encoding.ASCII.GetBytes("owned non-meeting fixture " + name);
            members.Add(name,new(bytes.Length,Sha(bytes)));
            writer.WriteEntry(new UstarTarEntry(TarEntryType.RegularFile, id + "/" + name)
                { DataStream = new MemoryStream(bytes) });
        }
        if (symlink) writer.WriteEntry(new UstarTarEntry(TarEntryType.SymbolicLink, id + "/ref")
            { LinkName = "../foreign" });
    }
    var size = tar.Length;
    tar.Position = 0;
    using var compressed = new MemoryStream();
    using (var zipper = new BZip2OutputStream(compressed) { IsStreamOwner = false }) tar.CopyTo(zipper);
    var archive = compressed.ToArray();
    return (new(id, archive.Length, Sha(archive), size, members.Count + (symlink ? 2 : 1), 0, members), archive);
}
static Process Spawn(params string[] args)
{
    var exe = Environment.ProcessPath ?? throw new InvalidOperationException();
    var info = new ProcessStartInfo(exe) { UseShellExecute = false, CreateNoWindow = true };
    if (Path.GetFileNameWithoutExtension(exe).Equals("dotnet", StringComparison.OrdinalIgnoreCase))
        info.ArgumentList.Add(typeof(ModelTransport).Assembly.Location);
    foreach (var arg in args) info.ArgumentList.Add(arg);
    return Process.Start(info) ?? throw new InvalidOperationException();
}
static async Task WaitMarker(Process child, string path)
{
    var watch = Stopwatch.StartNew();
    while (!File.Exists(path) && !child.HasExited && watch.Elapsed < TimeSpan.FromSeconds(15))
        await Task.Delay(25);
    Check(File.Exists(path) && !child.HasExited);
}
static async Task Install(string package, CancellationToken token, Action<string,string>? pre = null,
    Action? post = null, Action? nativeRace = null)
{
    var (pins, bytes) = Fixture();
    string models = Path.Combine(package, "models");
    Directory.CreateDirectory(models);
    using var lease = WindowsModelPaths.Acquire(package);
    using var folder = WindowsModelPaths.OpenChild(lease.Root, "models");
    using var stage = WindowsModelPaths.NewDirectory(folder, ".sig4a-en-" + Guid.NewGuid().ToString("N"), privateStage: true);
    await EnglishModelInstaller.StagedInstall(stage, folder, pins,
        (data, ct) => data.WriteAsync(bytes.AsMemory(), ct).AsTask(), _ => { }, token, pre, post, nativeRace);
}

static void Resolve(string package, EnglishModelPins.Manifest pins)
{
    using var lease = WindowsModelPaths.Acquire(package);
    using var model = EnglishModelInstaller.OpenInstalled(lease.Root, pins, CancellationToken.None);
}

static WindowsModelPaths.Identity IdentityAt(string parent, string leaf, bool directory = false)
{
    using var home = new WindowsModelPaths.Chain(parent);
    if (directory)
    {
        using var ownedDirectory = WindowsModelPaths.OpenChild(home.Root, leaf);
        return ownedDirectory.Id;
    }
    // Read the ID and release immediately; keeping a read handle through the
    // collision would independently block an accidental replace.
    using var item = WindowsModelPaths.OpenData(home.Root, leaf);
    return WindowsModelPaths.Inspect(item, Path.Combine(parent,leaf), directory: false);
}

static void Junction(string link, string target)
{
    var p = new ProcessStartInfo("powershell.exe") { UseShellExecute = false, CreateNoWindow = true };
    foreach (var arg in new[] { "-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; " +
        "New-Item -ItemType Junction -Path '" + link.Replace("'", "''") +
        "' -Target '" + target.Replace("'", "''") + "' | Out-Null" }) p.ArgumentList.Add(arg);
    using var child = Process.Start(p) ?? throw new InvalidOperationException();
    Check(child.WaitForExit(5000) && child.ExitCode == 0);
}

if (args is ["--race", var parent, var payload])
{
    var packageRoot = Directory.GetParent(parent)!.FullName;
    await Blocks(() => Directory.Move(packageRoot, packageRoot + "-unsafe"));
    await Blocks(() => Directory.Move(parent, parent + "-unsafe"));
    await Blocks(() => Directory.Move(payload, payload + "-unsafe"));
    await Blocks(() => File.AppendAllText(Path.Combine(payload, EnglishModelPins.Tokens), "foreign fixture edit"));
    await Blocks(() => File.Delete(Path.Combine(payload, EnglishModelPins.Tokens)));
    return;
}
if (args is ["--create-target", var modelHome, var createType])
{
    if (createType is "ptr" or "read-only")
    {
        var foreign = Path.Combine(modelHome, EnglishModelInstaller.MarkerLeaf);
        File.WriteAllText(foreign, "foreign");
        if (createType == "read-only") File.SetAttributes(foreign, FileAttributes.ReadOnly);
    }
    else
    {
        var legacyPath = Directory.CreateDirectory(Path.Combine(modelHome, EnglishModelInstaller.LegacyLeaf)).FullName;
        if (createType == "legacy-sentinel")
            File.WriteAllText(Path.Combine(legacyPath, "foreign.txt"), "legacy evidence");
    }
    return;
}
if (args is ["--kill", var childPackage, var childSignal, var phase])
{
    void Wait()
    {
        File.WriteAllText(childSignal, "child-held-native-guards");
        Thread.Sleep(Timeout.Infinite);
    }
    await Install(childPackage, CancellationToken.None, (_,_) => { if (phase == "before") Wait(); },
        () => { if (phase.StartsWith("after", StringComparison.Ordinal)) Wait(); });
    throw new InvalidOperationException("Fixture should have been killed.");
}

if (args is ["--check-official-archive", var file])
{
    using var original = File.OpenRead(file);
    var official = EnglishModelPins.Pins;
    Check(EnglishModelPins.Matches(original,new(official.ArchiveBytes,official.ArchiveHash)));
    original.Position = 0;
    EnglishModelExtract.VerifiedArchive(original,official,_=>Stream.Null,CancellationToken.None);
    Console.WriteLine("PASS: official exact archive and five member pins through actual managed extraction; no device.");
    return;
}

if (IntPtr.Size == 8)
{
    var nativeRoot = new IntPtr(0x0011223344556677);
    var rename = WindowsModelPaths.PointerRenameBuffer(nativeRoot);
    int n = BinaryPrimitives.ReadInt32LittleEndian(rename.AsSpan(16));
    Check(n == Encoding.Unicode.GetByteCount(EnglishModelInstaller.MarkerLeaf));
    Check(rename.Length == 24 + n && rename[..8].All(x => x == 0) &&
          BinaryPrimitives.ReadInt64LittleEndian(rename.AsSpan(8)) == nativeRoot.ToInt64() &&
          Encoding.Unicode.GetString(rename.AsSpan(20,n)) == EnglishModelInstaller.MarkerLeaf &&
          rename[(20+n)..].All(x => x == 0));
}
var (pinned, data) = Fixture();
EnglishModelExtract.VerifiedArchive(new MemoryStream(data), pinned, _ => new MemoryStream(), CancellationToken.None);
await Fails(() => Task.Run(() =>
{
    var (bad, raw) = Fixture(symlink:true);
    EnglishModelExtract.VerifiedArchive(new MemoryStream(raw), bad, _ => new MemoryStream(), CancellationToken.None);
}), "archive_invalid");
await Fails(() => Task.Run(() => EnglishModelExtract.VerifiedArchive(new MemoryStream(data),
    pinned with { ExpandedBytes = pinned.ExpandedBytes - 1 }, _ => new MemoryStream(), CancellationToken.None)),
    "archive_invalid");
Check(EnglishModelInstaller.DecodePointer(new MemoryStream(
    Encoding.ASCII.GetBytes("SIG4A-EN-1 0123456789abcdef0123456789abcdef\n"))) ==
    ".sig4a-en-0123456789abcdef0123456789abcdef");
foreach (var invalid in new[] {"SIG4A-EN-1 ../target", "SIG4A-EN-1 0123456789ABCDEF0123456789ABCDEF\n",
                               "SIG4A-EN-1 0123456789abcdef0123456789abcdef\r\n"})
    await Fails(() => Task.Run(() => EnglishModelInstaller.DecodePointer(new MemoryStream(Encoding.ASCII.GetBytes(invalid)))),
        "needs_repair");
Check(EnglishSetupMode.Parse(Encoding.ASCII.GetBytes("""{"v":1,"type":"model_setup_start","session":8,"action":"verify"}"""),false).Action == "verify");
foreach (var invalid in new[] { """{"v":1,"type":"model_setup_start","session":8,"action":"install","url":"https://other.invalid"}""",
                                  """{"v":1,"type":"model_setup_start","session":8,"action":"install","v":1}""" })
{
    try { _ = EnglishSetupMode.Parse(Encoding.ASCII.GetBytes(invalid), false); throw new InvalidOperationException("Bad setup frame accepted."); }
    catch (FormatException) { }
}

var cdn = new Uri("https://release-assets.githubusercontent.com/a?signed=fixture");
using (var handler = new ModelTransport(new HttpResponseMessage(HttpStatusCode.Found) { Headers = { Location = cdn } },
    new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(data) }))
using (var http = new HttpClient(handler))
using (var scratch = new MemoryStream())
{
    await EnglishModelDownload.FetchVerified(http, scratch, pinned, _ => {}, CancellationToken.None);
    Check(scratch.ToArray().SequenceEqual(data) && handler.Sent.SequenceEqual([EnglishModelPins.Origin,cdn]));
}
foreach (var target in new[] {"http://release-assets.githubusercontent.com/a", "https://release-assets.githubusercontent.com:444/a",
    "https://release-assets.githubusercontent.com.evil.invalid/a", "https://u@release-assets.githubusercontent.com/a"})
{
    using var handler = new ModelTransport(new HttpResponseMessage(HttpStatusCode.TemporaryRedirect)
        { Headers = { Location = new Uri(target) } });
    using var http = new HttpClient(handler);
    using var scratch = new MemoryStream();
    await Fails(() => EnglishModelDownload.FetchVerified(http, scratch,pinned, _ => {}, CancellationToken.None), "network_unavailable");
    Check(handler.Sent.Count == 1);
}
using (var handler = new ModelTransport(new HttpResponseMessage(HttpStatusCode.OK)
      { Content = new ByteArrayContent(data[..^1]) }))
using (var http = new HttpClient(handler))
using (var scratch = new MemoryStream())
    await Fails(() => EnglishModelDownload.FetchVerified(http,scratch,pinned,_=>{},CancellationToken.None),"digest_mismatch");
Console.WriteLine("PASS: exact model-only protocol, one CDN handoff, bounded verified archive, malformed pointer and safe extraction.");

if (OperatingSystem.IsWindows())
{
    string root = Path.Combine(Path.GetTempPath(), "sig4a-install-native-" + Guid.NewGuid().ToString("N"));
    Directory.CreateDirectory(root);
    try
    {
        string Make(string leaf) { var path = Path.Combine(root,leaf); Directory.CreateDirectory(path); return path; }
        var package = Make("success");
        await Install(package, CancellationToken.None, (parent,payload) =>
        {
            using var race = Spawn("--race",parent,payload);
            Check(race.WaitForExit(5000) && race.ExitCode == 0);
            try { using var unowned = WindowsModelPaths.Acquire(package); throw new InvalidOperationException("Package lease not exclusive."); }
            catch (SetupFailure e) when (e.Code == "busy") {}
        });
        Resolve(package,pinned);
        foreach (var type in new[] {"ptr","legacy"})
        {
            var conflict = Make("conflict-" + type);
            await Fails(() => Install(conflict,CancellationToken.None,(parent,_) =>
            {
                using var writer = Spawn("--create-target",parent,type);
                Check(writer.WaitForExit(5000) && writer.ExitCode == 0);
            }),"needs_repair");
            if (type == "ptr") Check(File.ReadAllText(Path.Combine(conflict,"models",EnglishModelInstaller.MarkerLeaf)) == "foreign");
            else Check(Directory.Exists(Path.Combine(conflict,"models",EnglishModelInstaller.LegacyLeaf)));
        }
        foreach (var type in new[] { "ptr", "read-only" })
        {
            var atomic = Make("competing-native-marker-" + type);
            var home = Path.Combine(atomic, "models");
            var prior = Path.Combine(home, EnglishModelInstaller.MarkerLeaf);
            WindowsModelPaths.Identity? original = null;
            byte[]? originalBytes = null;
            bool isReadOnly = type == "read-only";
            try
            {
                try
                {
                    await Install(atomic,CancellationToken.None,nativeRace: () =>
                    {
                        // Runs after the final absence probe, and before the OS
                        // rename. No open target handles may mask overwriting.
                        using var writer = Spawn("--create-target",home,type);
                        Check(writer.WaitForExit(5000) && writer.ExitCode == 0);
                        if (!isReadOnly)
                        {
                            using var writable = File.Open(prior, FileMode.Open, FileAccess.ReadWrite, FileShare.None);
                            Check(writable.CanWrite);
                        }
                        original = IdentityAt(home, EnglishModelInstaller.MarkerLeaf);
                        originalBytes = File.ReadAllBytes(prior);
                        Check(File.GetAttributes(prior).HasFlag(FileAttributes.ReadOnly) == isReadOnly);
                    });
                    throw new InvalidOperationException("Native marker rename overwrote its last-moment competitor.");
                }
                catch (System.ComponentModel.Win32Exception error) when
                    (error.NativeErrorCode is 80 or 183 || isReadOnly && error.NativeErrorCode == 5) {}
                Check(original is { } id && id == IdentityAt(home,EnglishModelInstaller.MarkerLeaf) &&
                    originalBytes is not null && originalBytes.AsSpan().SequenceEqual(File.ReadAllBytes(prior)) &&
                    File.GetAttributes(prior).HasFlag(FileAttributes.ReadOnly) == isReadOnly);
            }
            finally { if (isReadOnly && File.Exists(prior)) File.SetAttributes(prior,FileAttributes.Normal); }
        }

        // Once the real native rename has succeeded, insert a separate legacy
        // tree. Ordinary exception unwind must remove only our exact marker and
        // scratch, leaving the competing directory/file identities intact.
        var rollback = Make("post-publish-rollback");
        var rollbackHome = Path.Combine(rollback, "models");
        var foreignLegacy = Path.Combine(rollbackHome, EnglishModelInstaller.LegacyLeaf);
        var foreignFile = Path.Combine(foreignLegacy, "foreign.txt");
        WindowsModelPaths.Identity? originalLegacy = null, originalLegacyFile = null;
        await Fails(() => Install(rollback,CancellationToken.None,post: () =>
        {
            Check(Directory.EnumerateFileSystemEntries(rollbackHome)
                .Any(x => Path.GetFileName(x) == EnglishModelInstaller.MarkerLeaf));
            using var writer = Spawn("--create-target",rollbackHome,"legacy-sentinel");
            Check(writer.WaitForExit(5000) && writer.ExitCode == 0);
            originalLegacy = IdentityAt(rollbackHome,EnglishModelInstaller.LegacyLeaf,directory:true);
            originalLegacyFile = IdentityAt(foreignLegacy,"foreign.txt");
        }), "needs_repair");
        Check(Directory.GetFileSystemEntries(rollbackHome).SequenceEqual(new[] { foreignLegacy }) &&
            originalLegacy is { } d && d == IdentityAt(rollbackHome,EnglishModelInstaller.LegacyLeaf,directory:true) &&
            originalLegacyFile is { } f && f == IdentityAt(foreignLegacy,"foreign.txt") &&
            File.ReadAllText(foreignFile) == "legacy evidence");

        // Existing reparse components are not followed even when a legitimate
        // legacy install also exists. Only a truly absent marker permits legacy.
        var trap = Make("junk-target");
        var trapFile = Path.Combine(trap,"untouched.txt");
        File.WriteAllText(trapFile,"unrelated sentinel");
        foreach (var component in new[] { "models", "marker", "payload" })
        {
            var pointed = Make("reparse-" + component);
            var home = Path.Combine(pointed, "models");
            string pathToLink;
            if (component == "models") pathToLink = home;
            else
            {
                Directory.CreateDirectory(home);
                if (component == "marker")
                {
                    Directory.CreateDirectory(Path.Combine(home, EnglishModelInstaller.LegacyLeaf));
                    pathToLink = Path.Combine(home, EnglishModelInstaller.MarkerLeaf);
                }
                else
                {
                    const string stem = "01a23456789bcdef0123456789abcdef";
                    var slot = Directory.CreateDirectory(Path.Combine(home,EnglishModelInstaller.StagePrefix + stem)).FullName;
                    File.WriteAllText(Path.Combine(home,EnglishModelInstaller.MarkerLeaf),"SIG4A-EN-1 " + stem + "\n");
                    pathToLink = Path.Combine(slot,"payload");
                }
            }
            Junction(pathToLink,trap);
            try
            {
                await Fails(() => Task.Run(() => Resolve(pointed,pinned)), "invalid_path");
                Check(File.ReadAllText(trapFile) == "unrelated sentinel" && Directory.GetFileSystemEntries(trap).Length == 1);
            }
            finally { Directory.Delete(pathToLink); }
        }

        var manual = Make("legacy-with-unused-variants");
        var legacy = Directory.CreateDirectory(Path.Combine(manual,"models",EnglishModelInstaller.LegacyLeaf)).FullName;
        var (legacyPins, legacyBytes) = Fixture();
        EnglishModelExtract.VerifiedArchive(new MemoryStream(legacyBytes), legacyPins,
            name => File.Create(Path.Combine(legacy,name)),CancellationToken.None);
        File.WriteAllText(Path.Combine(legacy,"unused-precision.onnx"),"not loaded");
        File.Delete(Path.Combine(legacy,"README.md"));
        Resolve(manual,pinned);
        Check(File.ReadAllText(Path.Combine(legacy,"unused-precision.onnx")) == "not loaded");

        var cancelled = Make("cancel");
        using (var cts = new CancellationTokenSource())
        {
            try { await Install(cancelled,cts.Token,(_,_)=>cts.Cancel()); throw new InvalidOperationException("Cancel published."); }
            catch (OperationCanceledException) {}
            Check(!Directory.EnumerateFileSystemEntries(Path.Combine(cancelled,"models")).Any());
        }
        foreach (var when in new[] {"before","after","after-conflict"})
        {
            var crash = Make("kill-" + when);
            string signal = Path.Combine(root, "signalled-" + when);
            using (var child = Spawn("--kill",crash,signal,when))
            {
                try
                {
                    await WaitMarker(child,signal);
                    if (when == "after-conflict")
                    {
                        using var foreignProcess = Spawn("--create-target", Path.Combine(crash,"models"), "legacy");
                        Check(foreignProcess.WaitForExit(5000) && foreignProcess.ExitCode == 0);
                    }
                }
                finally { if (!child.HasExited) { child.Kill(true); child.WaitForExit(); } }
            }
            if (when == "after")
            {
                Resolve(crash,pinned);
            }
            else if (when == "after-conflict")
                await Fails(() => Task.Run(() => Resolve(crash,pinned)),"needs_repair");
            else
            {
                await Fails(() => Task.Run(() => Resolve(crash,pinned)),"not_installed");
                var counterfeit = Path.Combine(crash,"models",".sig4a-en-ffffffffffffffffffffffffffffffff");
                Directory.CreateDirectory(counterfeit);
                var foreign = Path.Combine(counterfeit,"foreign.txt");
                File.WriteAllText(foreign,"unrelated");
                await Install(crash,CancellationToken.None);
                Check(File.ReadAllText(foreign) == "unrelated");
                Resolve(crash,pinned);
            }
        }
        Console.WriteLine("PASS: Windows native rooted reparse rejection, legacy compatibility, actual no-overwrite races, exact owned rollback, anchored lease, cancellation and forced-kill recovery.");
    }
    finally { Directory.Delete(root, recursive:true); } // Only test-owned temp, no production cleanup API.
}
else Console.WriteLine("SKIP: Windows/NTFS race fixtures cannot run on this OS.");

sealed class ModelTransport(params HttpResponseMessage[] scripts) : HttpMessageHandler
{
    internal readonly List<Uri> Sent = [];
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        if (request.RequestUri is not { } uri || request.Method != HttpMethod.Get ||
            request.Content is not null || request.Headers.Authorization is not null)
            throw new InvalidOperationException("Unexpected setup fixture network request.");
        Sent.Add(uri);
        return Task.FromResult(scripts[Sent.Count - 1]);
    }
}
