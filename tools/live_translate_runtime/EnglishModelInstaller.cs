using System.ComponentModel;
using System.Text;

static class EnglishModelInstaller
{
    internal const string MarkerLeaf = "zipformer.current", LegacyLeaf = "zipformer", StagePrefix = ".sig4a-en-";
    const string PointerPrefix = "SIG4A-EN-1 ";
    const int PointerSize = 44;
    internal static readonly string Root = Path.GetFullPath(AppContext.BaseDirectory).TrimEnd(Path.DirectorySeparatorChar);

    internal sealed class VerifiedFiles : IDisposable
    {
        readonly Dictionary<string,FileStream> pins = new(StringComparer.Ordinal);
        internal readonly Dictionary<string, WindowsModelPaths.Identity> Ids = new(StringComparer.Ordinal);
        internal string NativeName(string name) => WindowsModelPaths.NativeAssetName(pins[name].SafeFileHandle);
        internal void Hold(string name, FileStream asset, WindowsModelPaths.Identity id)
        {
            pins.Add(name, asset);
            Ids.Add(name, id);
        }
        internal void Confirm(WindowsModelPaths.Pin directory, EnglishModelPins.Manifest model, CancellationToken stop)
        {
            var found = new HashSet<string>(StringComparer.Ordinal);
            foreach (var name in WindowsModelPaths.Members(directory, pins.Count))
            {
                string path = Path.Combine(directory.Path, name);
                if (!pins.TryGetValue(name, out var data) || !found.Add(name) ||
                    WindowsModelPaths.Inspect(data.SafeFileHandle, path, directory: false) != Ids[name])
                    throw new SetupFailure("needs_repair");
                data.Position = 0;
                if (!EnglishModelPins.Matches(data, model.Members[name], stop)) throw new SetupFailure("needs_repair");
            }
            if (found.Count != pins.Count) throw new SetupFailure("needs_repair");
        }
        public void Dispose()
        {
            foreach (var pin in pins.Values) pin.Dispose();
            pins.Clear();
        }
    }

    internal sealed class AssetLease(List<IDisposable> handles, VerifiedFiles files) : IDisposable
    {
        internal string NativeName(string name) => files.NativeName(name);
        public void Dispose()
        {
            files.Dispose();
            for (int i = handles.Count - 1; i >= 0; i--) handles[i].Dispose();
        }
    }

    internal static string DecodePointer(Stream marker)
    {
        if (marker.Length != PointerSize) throw new SetupFailure("needs_repair");
        marker.Position = 0;
        Span<byte> bytes = stackalloc byte[PointerSize];
        marker.ReadExactly(bytes);
        if (!bytes[..PointerPrefix.Length].SequenceEqual(Encoding.ASCII.GetBytes(PointerPrefix)) ||
            bytes[^1] != (byte)'\n') throw new SetupFailure("needs_repair");
        var name = bytes.Slice(PointerPrefix.Length, 32);
        foreach (var value in name)
            if (value is not ((>= (byte)'a' and <= (byte)'f') or (>= (byte)'0' and <= (byte)'9')))
                throw new SetupFailure("needs_repair");
        return StagePrefix + Encoding.ASCII.GetString(name);
    }

    internal static AssetLease OpenInstalled(WindowsModelPaths.Pin root, EnglishModelPins.Manifest pins, CancellationToken stop)
    {
        var held = new List<IDisposable>();
        T Keep<T>(T item) where T : IDisposable { held.Add(item); return item; }
        var models = WindowsModelPaths.Child(root, "models");
        if (models == WindowsModelPaths.Entry.Missing) throw new SetupFailure("not_installed");
        if (models != WindowsModelPaths.Entry.Directory) throw new SetupFailure("invalid_path");
        try
        {
            var parent = Keep(WindowsModelPaths.OpenChild(root, "models"));
            var pointer = WindowsModelPaths.Child(parent, MarkerLeaf);
            var legacy = WindowsModelPaths.Child(parent, LegacyLeaf);
            if (pointer == WindowsModelPaths.Entry.Missing && legacy == WindowsModelPaths.Entry.Missing)
                throw new SetupFailure("not_installed");
            if (pointer == WindowsModelPaths.Entry.Directory || legacy == WindowsModelPaths.Entry.File ||
                pointer != WindowsModelPaths.Entry.Missing && legacy != WindowsModelPaths.Entry.Missing)
                throw new SetupFailure("needs_repair");
            WindowsModelPaths.Pin model;
            bool isPointer = pointer == WindowsModelPaths.Entry.File;
            if (isPointer)
            {
                var marker = Keep(new FileStream(WindowsModelPaths.OpenData(parent, MarkerLeaf), FileAccess.Read));
                var stage = Keep(WindowsModelPaths.OpenChild(parent, DecodePointer(marker)));
                model = Keep(WindowsModelPaths.OpenChild(stage, "payload"));
            }
            else model = Keep(WindowsModelPaths.OpenChild(parent, LegacyLeaf));
            var assets = Verify(model, pins, stop, exactMembership: isPointer);
            try
            {
                // Both formats together are never resolved, including after a
                // force-kill just after marker publication.
                if (WindowsModelPaths.Child(parent, MarkerLeaf) != pointer ||
                    WindowsModelPaths.Child(parent, LegacyLeaf) != legacy)
                    throw new SetupFailure("needs_repair");
                return new AssetLease(held, assets);
            }
            catch { assets.Dispose(); throw; }
        }
        catch
        {
            for (int i = held.Count - 1; i >= 0; i--) held[i].Dispose();
            throw; // uncertain pointer cannot select legacy.
        }
    }

    internal static VerifiedFiles Verify(WindowsModelPaths.Pin directory, EnglishModelPins.Manifest pins,
        CancellationToken stop, bool exactMembership = true)
    {
        var guarded = new VerifiedFiles();
        try
        {
            var expected = pins.Members;
            var found = new HashSet<string>(StringComparer.Ordinal);
            // Older manually extracted packages can also contain unused ONNX
            // variants and docs. Only open the four pinned runtime files.
            var files = exactMembership ? WindowsModelPaths.Members(directory, expected.Count) :
                EnglishModelPins.ModelFiles.AsEnumerable();
            foreach (string name in files)
            {
                stop.ThrowIfCancellationRequested();
                if (!expected.TryGetValue(name, out var pin) || !found.Add(name))
                    throw new SetupFailure("needs_repair");
                var handle = WindowsModelPaths.OpenData(directory, name);
                FileStream? asset = null;
                try
                {
                    asset = new FileStream(handle, FileAccess.Read);
                    var id = WindowsModelPaths.Inspect(handle, Path.Combine(directory.Path, name), directory: false);
                    if (!EnglishModelPins.Matches(asset, pin, stop)) throw new SetupFailure("needs_repair");
                    guarded.Hold(name, asset, id);
                }
                catch
                {
                    if (asset is null) handle.Dispose();
                    else asset.Dispose();
                    throw;
                }
            }
            if (exactMembership && found.Count != expected.Count) throw new SetupFailure("needs_repair");
            return guarded;
        }
        catch { guarded.Dispose(); throw; }
    }

    internal static async Task Execute(string action, Action<string> stage, Action<long> progress, CancellationToken stop)
    {
        var pins = EnglishModelPins.Pins;
        using var lease = WindowsModelPaths.Acquire(Root);
        var root = lease.Root;
        stop.ThrowIfCancellationRequested();
        stage("checking_files");
        try
        {
            using var already = OpenInstalled(root, pins, stop);
            stop.ThrowIfCancellationRequested();
            return; // Already verified: no network, inference, or overwrite.
        }
        catch (SetupFailure e) when (e.Code == "not_installed" && action == "install") { }
        if (new DriveInfo(Path.GetPathRoot(Root)!).AvailableFreeSpace < pins.MinimumFree)
            throw new SetupFailure("insufficient_space");
        using var modelsGuard = WindowsModelPaths.Child(root, "models") == WindowsModelPaths.Entry.Missing
            ? WindowsModelPaths.NewDirectory(root, "models", privateStage: false)
            : WindowsModelPaths.OpenChild(root, "models");
        using var workspace = WindowsModelPaths.NewDirectory(modelsGuard,
            StagePrefix + Guid.NewGuid().ToString("N"), privateStage: true);
        await StagedInstall(workspace, modelsGuard, pins,
            (stream, token) => EnglishModelDownload.Fetch(stream, progress, token), stage, stop);
    }

    // Tests can supply a synthetic archive and a prepublish competing-process
    // hook. Production passes only pinned bytes and never exposes these hooks in IPC.
    internal static async Task StagedInstall(WindowsModelPaths.Pin workspace, WindowsModelPaths.Pin modelsGuard,
        EnglishModelPins.Manifest pins, Func<Stream, CancellationToken, Task> fetch,
        Action<string> status, CancellationToken stop, Action<string,string>? prepublish = null, Action? publishedHook = null, Action? atomicRace = null)
    {
        var work = workspace.Path;
        var payload = Path.Combine(work, "payload");
        var markerStage = Path.Combine(work, "marker.part");
        var markerTarget = Path.Combine(modelsGuard.Path, MarkerLeaf);
        var created = new Dictionary<string, WindowsModelPaths.Identity>(StringComparer.Ordinal);
        WindowsModelPaths.Pin? source = null;
        FileStream? marker = null;
        bool published = false, committed = false;
        try
        {
            stop.ThrowIfCancellationRequested();
            source = WindowsModelPaths.NewDirectory(workspace, "payload", privateStage: true);
            using (var archive = WindowsModelPaths.NewData(workspace, "archive.part", deleteOnClose: true))
            {
                await fetch(archive, stop);
                status("checking_archive");
                archive.Position = 0;
                if (!EnglishModelPins.Matches(archive, new(pins.ArchiveBytes, pins.ArchiveHash), stop))
                    throw new SetupFailure("digest_mismatch");
                archive.Position = 0;
                status("extracting");
                EnglishModelExtract.VerifiedArchive(archive, pins, name =>
                {
                    var path = Path.Combine(payload, name);
                    var output = WindowsModelPaths.NewData(source, name);
                    try
                    {
                        created.Add(name, WindowsModelPaths.Inspect(output.SafeFileHandle, path, directory: false));
                        return output;
                    }
                    catch { output.Dispose(); throw; }
                }, stop);
            }

            status("checking_files");
            using var held = Verify(source, pins, stop); // Data guards survive atomic marker publication.
            foreach (var (name, identity) in held.Ids)
                if (!created.TryGetValue(name, out var createdId) || identity != createdId)
                    throw new SetupFailure("needs_repair");

            var stageName = Path.GetFileName(workspace.Path);
            if (!stageName.StartsWith(StagePrefix, StringComparison.Ordinal))
                throw new SetupFailure("invalid_path");
            marker = WindowsModelPaths.NewData(workspace, "marker.part");
            var markerId = WindowsModelPaths.Inspect(marker.SafeFileHandle, markerStage, directory: false);
            marker.Write(Encoding.ASCII.GetBytes(PointerPrefix + stageName[StagePrefix.Length..] + "\n"));
            marker.Flush(flushToDisk: true);
            if (DecodePointer(marker) != stageName) throw new SetupFailure("invalid_path");

            prepublish?.Invoke(modelsGuard.Path, payload);
            status("committing");
            WindowsModelPaths.ConfirmPin(modelsGuard);
            WindowsModelPaths.ConfirmPin(workspace);
            WindowsModelPaths.ConfirmPin(source);
            if (WindowsModelPaths.Child(modelsGuard, MarkerLeaf) != WindowsModelPaths.Entry.Missing ||
                WindowsModelPaths.Child(modelsGuard, LegacyLeaf) != WindowsModelPaths.Entry.Missing)
                throw new SetupFailure("needs_repair");
            stop.ThrowIfCancellationRequested();
            atomicRace?.Invoke(); // Native no-overwrite fixture runs after final probes.
            WindowsModelPaths.CommitPointer(marker, markerStage, markerId, modelsGuard);
            published = true;
            publishedHook?.Invoke();
            if (WindowsModelPaths.Inspect(marker.SafeFileHandle, markerTarget, directory: false) != markerId ||
                DecodePointer(marker) != stageName ||
                WindowsModelPaths.Child(modelsGuard, LegacyLeaf) != WindowsModelPaths.Entry.Missing)
                throw new SetupFailure("needs_repair");
            WindowsModelPaths.ConfirmPin(workspace);
            WindowsModelPaths.ConfirmPin(source);
            held.Confirm(source, pins, CancellationToken.None);
            committed = true; // Fully verified pointer won; cancellation no longer rolls it back.
        }
        finally
        {
            // Recover only exact live-process handles/identities. A job/host kill
            // may leave our uniquely named scratch payload (archive is delete-on-close).
            // Later attempts never read mutable ownership journals or remove old trees.
            bool clean = true;
            if (marker is not null)
            {
                if (!committed)
                {
                    try { WindowsModelPaths.DeleteData(marker); }
                    catch (Exception) { clean = false; }
                }
                marker.Dispose();
            }
            if (source is not null)
            {
                // If exact published-marker rollback failed, keep its payload.
                if (!committed && (!published || clean))
                {
                    try
                    {
                        WindowsModelPaths.ConfirmPin(source);
                        foreach (var (name,id) in created)
                            WindowsModelPaths.RemoveOwnedFile(source,name, id);
                        source.RemoveKnownEmpty();
                    }
                    catch (Exception) { clean = false; }
                }
                source.Dispose();
            }
            if (!committed)
            {
                try { workspace.RemoveKnownEmpty(); }
                catch (Exception) { clean = false; }
            }
            if (!clean) throw new SetupFailure("cleanup_incomplete");
        }
    }

    internal static string ErrorCode(Exception x) => x switch
    {
        SetupFailure failure => failure.Code,
        UnauthorizedAccessException or Win32Exception { NativeErrorCode: 5 } => "access_denied",
        Win32Exception { NativeErrorCode: 112 or 39 } => "insufficient_space",
        Win32Exception { NativeErrorCode: 183 or 80 } => "needs_repair",
        DirectoryNotFoundException or FileNotFoundException or Win32Exception { NativeErrorCode: 2 or 3 or 32 or 1920 or 1921 or 4390 or 4395 } => "invalid_path",
        IOException io when (io.HResult & 0xffff) is 112 or 39 => "insufficient_space",
        _ => "installer_internal"
    };
}
