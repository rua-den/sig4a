using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text;
using Microsoft.Win32.SafeHandles;

static class WindowsModelPaths
{
    const uint ListDirectory = 1, Traverse = 0x20, ReadAttributes = 0x80, Delete = 0x10000;
    const uint ShareRead = 1, ShareWrite = 2, BackupSemantics = 0x02000000, OpenReparsePoint = 0x00200000;
    const uint DirectoryBit = 0x10, ReparseBit = 0x400;
    const uint OpenExisting = 3, GenericReadWrite = 0xC0000000, Sync = 0x00100000;
    const uint FileOpen = 1, FileCreate = 2, DirectoryFile = 1, NonDirectoryFile = 0x40,
        SyncNonalert = 0x20, DeleteOnClose = 0x1000, FileOpenReparsePoint = 0x00200000,
        ObjNoReparse = 0x1000, ObjCaseInsensitive = 0x40;
    const int WinAlreadyExists = 183;

    internal record struct Identity(ulong Volume, ulong Low, ulong High)
    {
        public override string ToString() => $"{Volume:X16}{Low:X16}{High:X16}";
    }

    internal sealed class Pin(string path, SafeFileHandle handle, Identity identity) : IDisposable
    {
        internal string Path { get; } = path;
        internal SafeFileHandle Handle { get; } = handle;
        internal Identity Id { get; } = identity;
        public void Dispose() => Handle.Dispose();
        internal void RemoveKnownEmpty()
        {
            var flag = Marshal.AllocHGlobal(1);
            try
            {
                Marshal.WriteByte(flag, 1);
                if (!SetFileInformationByHandle(Handle, 4, flag, 1)) throw Last();
            }
            finally { Marshal.FreeHGlobal(flag); }
        }
    }

    internal sealed class Chain : IDisposable
    {
        readonly List<Pin> handles = [];
        internal Pin Root => handles[^1];
        internal Chain(string directory)
        {
            try
            {
                var full = Path.GetFullPath(directory).TrimEnd(Path.DirectorySeparatorChar);
                if (full.Length < 3 || !char.IsAsciiLetter(full[0]) || full[1] != ':' || full[2] != '\\' ||
                    new DriveInfo(full[..3]).DriveType != DriveType.Fixed)
                    throw new SetupFailure("invalid_path");
                // Path text is never supplied by the IPC caller. Open each ancestor
                // from the stable local drive root while retaining the prior guard.
                var parts = full.Split('\\');
                var path = parts[0] + "\\";
                handles.Add(OpenDrive(path));
                foreach (var part in parts.Skip(1))
                {
                    if (part.Length == 0) throw new SetupFailure("invalid_path");
                    handles.Add(OpenChild(handles[^1], part));
                }
            }
            catch { Dispose(); throw; }
        }

        internal void Confirm()
        {
            foreach (var pin in handles) ConfirmPin(pin);
        }

        public void Dispose()
        {
            for (int i = handles.Count - 1; i >= 0; i--) handles[i].Dispose();
        }
    }

    static Pin OpenDrive(string path)
    {
        var file = CreateFileW(path, ListDirectory | Traverse | ReadAttributes, ShareRead | ShareWrite,
            IntPtr.Zero, OpenExisting, BackupSemantics | OpenReparsePoint, IntPtr.Zero);
        if (file.IsInvalid) { file.Dispose(); throw Last(); }
        try
        {
            var fs = new StringBuilder(64);
            if (!GetVolumeInformationByHandleW(file, IntPtr.Zero, 0, out _, out _, out _, fs, (uint)fs.Capacity))
                throw Last();
            if (!fs.ToString().Equals("NTFS", StringComparison.OrdinalIgnoreCase))
                throw new SetupFailure("invalid_path");
            return new(path, file, Inspect(file, path, directory: true));
        }
        catch { file.Dispose(); throw; }
    }

    internal static Pin OpenChild(Pin parent, string leaf, bool removable = false)
    {
        string path = LeafPath(parent, leaf);
        var file = Relative(parent, leaf, ListDirectory | Traverse | ReadAttributes | (removable ? Delete : 0),
            ShareRead | ShareWrite, FileOpen, DirectoryFile);
        try { return new(path, file, Inspect(file, path, directory: true)); }
        catch { file.Dispose(); throw; }
    }

    internal static void ConfirmPin(Pin pin)
    {
        if (Inspect(pin.Handle, pin.Path, directory: true) != pin.Id)
            throw new SetupFailure("invalid_path");
    }

    internal static Identity Inspect(SafeFileHandle handle, string path, bool directory)
    {
        if (!GetFileInformationByHandleEx(handle, 9, out AttributeTag tag, 8)) throw Last();
        if ((tag.Attributes & ReparseBit) != 0 || ((tag.Attributes & DirectoryBit) != 0) != directory)
            throw new SetupFailure("invalid_path");
        var name = new StringBuilder(1024);
        uint size = GetFinalPathNameByHandleW(handle, name, (uint)name.Capacity, 0);
        if (size == 0 || size >= name.Capacity) throw new SetupFailure("invalid_path");
        var result = name.ToString();
        if (!result.StartsWith(@"\\?\", StringComparison.Ordinal) ||
            !result[4..].TrimEnd('\\').Equals(path.TrimEnd('\\'), StringComparison.OrdinalIgnoreCase))
            throw new SetupFailure("invalid_path");
        if (!GetFileInformationByHandleEx(handle, 18, out NativeIdentifier id, 24)) throw Last();
        return new(id.Volume, id.Low, id.High);
    }

    internal enum Entry { Missing, Directory, File }

    // Native Sherpa opens named paths. Use volume-GUID paths from the actual
    // guarded files, independent of drive letter aliases while they are pinned.
    internal static string NativeAssetName(SafeFileHandle heldFile)
    {
        var n = new StringBuilder(1024);
        uint count = GetFinalPathNameByHandleW(heldFile, n, (uint)n.Capacity, 1); // VOLUME_NAME_GUID
        if (count == 0 || count >= n.Capacity || !n.ToString().StartsWith(@"\\?\Volume{", StringComparison.OrdinalIgnoreCase))
            throw new SetupFailure("invalid_path");
        return n.ToString();
    }

    static string LeafPath(Pin parent, string leaf)
    {
        if (leaf is "." or ".." || leaf.Length is < 1 or > 255 ||
            leaf.AsSpan().IndexOfAny('\\', '/', ':') >= 0 || leaf.Contains('\0') || leaf.EndsWith(' ') || leaf.EndsWith('.'))
            throw new SetupFailure("invalid_path");
        return Path.Combine(parent.Path, leaf);
    }

    // All descendant I/O, including create and membership listing, is rooted
    // at an already opened directory object rather than a repeat path traverse.
    // OBJ_DONT_REPARSE blocks NT redirection; OPEN_REPARSE_POINT plus Inspect
    // refuses even a final-component reparse returned without traversal.
    sealed class MissingLeaf : Exception { }
    static SafeFileHandle Relative(Pin parent, string leaf, uint access, uint share,
        uint disposition, uint options, IntPtr descriptor = default, bool detectAbsence = false)
    {
        _ = LeafPath(parent, leaf);
        ConfirmPin(parent);
        var text = Marshal.StringToHGlobalUni(leaf);
        var ptr = Marshal.AllocHGlobal(Marshal.SizeOf<UnicodeString>());
        bool held = false;
        try
        {
            parent.Handle.DangerousAddRef(ref held);
            var name = new UnicodeString { Length = (ushort)(leaf.Length * 2), MaxLength = (ushort)((leaf.Length + 1) * 2), Buffer = text };
            Marshal.StructureToPtr(name, ptr, false);
            var obj = new ObjectAttributes { Length = Marshal.SizeOf<ObjectAttributes>(),
                RootDirectory = parent.Handle.DangerousGetHandle(), Name = ptr,
                Attributes = ObjNoReparse | ObjCaseInsensitive, SecurityDescriptor = descriptor };
            int status = NtCreateFile(out var native, access | Sync, ref obj, out _, IntPtr.Zero,
                disposition == FileCreate ? 0x80u : 0u, share, disposition, options | SyncNonalert | FileOpenReparsePoint, IntPtr.Zero, 0);
            if (status < 0)
            {
                // These two statuses are positive leaf absence only when
                // returned directly by this anchored one-component open.
                if (detectAbsence && status is (unchecked((int)0xC0000034) or unchecked((int)0xC000000F)))
                    throw new MissingLeaf();
                throw new Win32Exception((int)RtlNtStatusToDosError(status));
            }
            return new SafeFileHandle(native, ownsHandle: true);
        }
        finally
        {
            if (held) parent.Handle.DangerousRelease();
            Marshal.FreeHGlobal(ptr);
            Marshal.FreeHGlobal(text);
        }
    }

    // Only a positively observed file-not-found under a guarded parent can
    // select a fallback. Sharing, reparse and unknown failures do not.
    internal static Entry Child(Pin parent, string leaf)
    {
        string path = LeafPath(parent, leaf);
        SafeFileHandle handle;
        try { handle = Relative(parent, leaf, ReadAttributes, 7, FileOpen, 0, detectAbsence: true); }
        catch (MissingLeaf) { return Entry.Missing; }
        using (handle)
        {
            if (!GetFileInformationByHandleEx(handle, 9, out AttributeTag tag, 8)) throw Last();
            bool dir = (tag.Attributes & DirectoryBit) != 0;
            _ = Inspect(handle, path, directory: dir);
            return dir ? Entry.Directory : Entry.File;
        }
    }

    internal static SafeFileHandle OpenData(Pin parent, string leaf)
    {
        string path = LeafPath(parent, leaf);
        var handle = Relative(parent, leaf, 0x80000000u, ShareRead, FileOpen, NonDirectoryFile);
        try { _ = Inspect(handle, path, directory: false); return handle; }
        catch { handle.Dispose(); throw; }
    }

    internal static List<string> Members(Pin parent, int maximum)
    {
        ConfirmPin(parent);
        var result = new List<string>();
        const int bytes = 8192, namesInformation = 12;
        var buffer = Marshal.AllocHGlobal(bytes);
        try
        {
            bool restart = true;
            while (true)
            {
                int status = NtQueryDirectoryFile(parent.Handle, IntPtr.Zero, IntPtr.Zero, IntPtr.Zero,
                    out var io, buffer, bytes, namesInformation, false, IntPtr.Zero, restart);
                restart = false;
                if (status is unchecked((int)0x80000006) or unchecked((int)0xC000000F)) return result;
                if (status < 0 || io.Information < 12 || io.Information > bytes)
                    throw new Win32Exception((int)RtlNtStatusToDosError(status));
                int limit = (int)io.Information;
                for (int offset = 0; ;)
                {
                    if (limit - offset < 12) throw new SetupFailure("invalid_path");
                    int next = Marshal.ReadInt32(buffer, offset);
                    int size = Marshal.ReadInt32(buffer, offset + 8);
                    if (size < 2 || size % 2 != 0 || size > limit - offset - 12)
                        throw new SetupFailure("invalid_path");
                    string leaf = Marshal.PtrToStringUni(buffer + offset + 12, size / 2) ?? throw new SetupFailure("invalid_path");
                    if (leaf is not ("." or ".."))
                    {
                        _ = LeafPath(parent, leaf);
                        if (result.Count == maximum) throw new SetupFailure("needs_repair");
                        result.Add(leaf);
                    }
                    if (next == 0) break;
                    if (next < 12 + size || next > limit - offset) throw new SetupFailure("invalid_path");
                    offset += next;
                }
            }
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    // Private ACL applied on creation of fresh owned stage only; never updates
    // the parent or transfers ownership to a different identity.
    internal static Pin NewDirectory(Pin parent, string leaf, bool privateStage)
    {
        string path = LeafPath(parent, leaf);
        IntPtr native = IntPtr.Zero;
        try
        {
            if (privateStage)
            {
                using var identity = WindowsIdentity.GetCurrent();
                var sid = identity.User?.Value ?? throw new SetupFailure("access_denied");
                var acl = $"D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)";
                if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(acl, 1, out native, out _)) throw Last();
            }
            var file = Relative(parent, leaf, ListDirectory | Traverse | ReadAttributes | (privateStage ? Delete : 0),
                ShareRead | ShareWrite, FileCreate, DirectoryFile, native);
            try { return new(path, file, Inspect(file, path, directory: true)); }
            catch { file.Dispose(); throw; }
        }
        finally { if (native != IntPtr.Zero) _ = LocalFree(native); }
    }

    internal static FileStream NewData(Pin parent, string leaf, bool deleteOnClose = false)
    {
        string path = LeafPath(parent, leaf);
        var handle = Relative(parent, leaf, GenericReadWrite | Delete, ShareRead, FileCreate,
            NonDirectoryFile | (deleteOnClose ? DeleteOnClose : 0));
        try { _ = Inspect(handle, path, directory: false); return new FileStream(handle, FileAccess.ReadWrite, 64 * 1024, false); }
        catch { handle.Dispose(); throw; }
    }

    internal static void RemoveOwnedFile(Pin parent, string leaf, Identity identity)
    {
        string path = LeafPath(parent, leaf);
        using var handle = Relative(parent, leaf, Delete | ReadAttributes, ShareRead, FileOpen, NonDirectoryFile);
        if (Inspect(handle, path, directory: false) != identity)
            throw new SetupFailure("cleanup_incomplete");
        FlagDelete(handle);
    }

    internal static void DeleteData(FileStream stream) => FlagDelete(stream.SafeFileHandle);

    static void FlagDelete(SafeFileHandle handle)
    {
        var buffer = Marshal.AllocHGlobal(1);
        try
        {
            Marshal.WriteByte(buffer, 1);
            if (!SetFileInformationByHandle(handle, 4, buffer, 1)) throw Last();
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    internal static void CommitPointer(FileStream original, string source, Identity identity, Pin parent)
    {
        const string leaf = EnglishModelInstaller.MarkerLeaf;
        var handle = original.SafeFileHandle;
        if (Inspect(handle, source, directory: false) != identity || identity.Volume != parent.Id.Volume || IntPtr.Size != 8)
            throw new SetupFailure("invalid_path");
        ConfirmPin(parent);
        var name = Encoding.Unicode.GetBytes(leaf + "\0");
        // FILE_RENAME_INFO x64: zero no-overwrite BOOLEAN/union, HANDLE at
        // offset 8, ULONG text byte count at 16, UTF-16 text at offset 20.
        int bytes = 24 + name.Length;
        var info = Marshal.AllocHGlobal(bytes);
        try
        {
            Marshal.WriteInt64(info, 0, 0);
            Marshal.WriteIntPtr(info, 8, parent.Handle.DangerousGetHandle());
            Marshal.WriteInt32(info, 16, name.Length - 2);
            Marshal.Copy(name, 0, info + 20, name.Length);
            if (!SetFileInformationByHandle(handle, 3, info, (uint)bytes)) throw Last();
        }
        finally { Marshal.FreeHGlobal(info); }
    }

    internal sealed class PackageLease(Chain chain, SafeWaitHandle mutex) : IDisposable
    {
        internal Pin Root => chain.Root;
        public void Dispose()
        {
            // Retain anchoring through mutex release, not only while deriving ID.
            mutex.Dispose();
            chain.Dispose();
        }
    }

    internal static PackageLease Acquire(string directory)
    {
        var chain = new Chain(directory);
        try { return Acquire(chain); }
        catch { chain.Dispose(); throw; }
    }

    static PackageLease Acquire(Chain chain)
    {
        var name = @"Global\SIG4A.EnglishModel.Package.v1." + chain.Root.Id;
        // Object existence (not mutex ownership) is the lease; handles can safely
        // cross async threads and the kernel drops this lease on process death.
        var handle = CreateMutexW(IntPtr.Zero, false, name);
        var code = Marshal.GetLastPInvokeError();
        if (handle.IsInvalid || code == WinAlreadyExists)
        {
            handle.Dispose();
            throw new SetupFailure(code == WinAlreadyExists ? "busy" : "access_denied");
        }
        return new PackageLease(chain, handle);
    }

    static Win32Exception Last() => new(Marshal.GetLastPInvokeError());
    [StructLayout(LayoutKind.Sequential)] struct UnicodeString { internal ushort Length, MaxLength; internal IntPtr Buffer; }
    [StructLayout(LayoutKind.Sequential)] struct ObjectAttributes
    { internal int Length; internal IntPtr RootDirectory, Name; internal uint Attributes; internal IntPtr SecurityDescriptor, SecurityQos; }
    [StructLayout(LayoutKind.Sequential)] struct IoStatus { internal IntPtr Status; internal nuint Information; }
    [DllImport("ntdll.dll", ExactSpelling = true)]
    static extern int NtCreateFile(out IntPtr handle, uint access, ref ObjectAttributes attributes, out IoStatus io,
        IntPtr allocationSize, uint attributesOfFile, uint share, uint disposition, uint options, IntPtr data, uint dataLength);
    [DllImport("ntdll.dll", ExactSpelling = true)]
    static extern uint RtlNtStatusToDosError(int status);
    [DllImport("ntdll.dll", ExactSpelling = true)]
    static extern int NtQueryDirectoryFile(SafeFileHandle handle, IntPtr evt, IntPtr apc, IntPtr apcContext,
        out IoStatus io, IntPtr data, int dataSize, int infoClass, [MarshalAs(UnmanagedType.U1)] bool single,
        IntPtr pattern, [MarshalAs(UnmanagedType.U1)] bool restart);
    [StructLayout(LayoutKind.Sequential)] struct AttributeTag { internal uint Attributes, Tag; }
    [StructLayout(LayoutKind.Sequential)] struct NativeIdentifier { internal ulong Volume, Low, High; }

    [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true, ExactSpelling = true)]
    static extern SafeFileHandle CreateFileW(string filename, uint access, uint share, IntPtr security, uint creation, uint flags, IntPtr template);
    [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true, ExactSpelling = true)]
    static extern SafeWaitHandle CreateMutexW(IntPtr security, [MarshalAs(UnmanagedType.Bool)] bool owned, string name);
    [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true, ExactSpelling = true)]
    static extern uint GetFinalPathNameByHandleW(SafeFileHandle handle, StringBuilder name, uint capacity, uint flags);
    [DllImport("kernel32", CharSet = CharSet.Unicode, SetLastError = true, ExactSpelling = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetVolumeInformationByHandleW(SafeFileHandle file, IntPtr volumeName, uint volumeLength,
        out uint serial, out uint componentSize, out uint flags, StringBuilder fileSystem, uint fileSystemLength);
    [DllImport("kernel32", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int info, out AttributeTag result, uint bytes);
    [DllImport("kernel32", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle handle, int info, out NativeIdentifier result, uint bytes);
    [DllImport("kernel32", SetLastError = true)] [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool SetFileInformationByHandle(SafeFileHandle handle, int info, IntPtr data, uint bytes);
    [DllImport("advapi32", CharSet = CharSet.Unicode, SetLastError = true, ExactSpelling = true)] [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string descriptor, uint revision, out IntPtr result, out uint bytes);
    [DllImport("kernel32")] static extern IntPtr LocalFree(IntPtr p);
}
