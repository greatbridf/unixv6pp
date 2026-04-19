#ifndef FILE_MANAGER_H
#define FILE_MANAGER_H

#include "OpenFileManager.h"
#include "File.h"

class FileManager;

extern "C" void FileManager_initialize(FileManager*);
extern "C" void FileManager_open(FileManager*);
extern "C" void FileManager_creat(FileManager*);
extern "C" void FileManager_open1(FileManager*, Inode*, int, int);
extern "C" void FileManager_close(FileManager*);
extern "C" void FileManager_seek(FileManager*);
extern "C" void FileManager_dup(FileManager*);
extern "C" void FileManager_fstat(FileManager*);
extern "C" void FileManager_stat(FileManager*);
extern "C" void FileManager_stat1(FileManager*, Inode*, unsigned long);
extern "C" void FileManager_read(FileManager*);
extern "C" void FileManager_write(FileManager*);
extern "C" void FileManager_rdwr(FileManager*, unsigned int mode);
extern "C" void FileManager_pipe(FileManager*);
extern "C" void FileManager_readp(FileManager*, File*);
extern "C" void FileManager_writep(FileManager*, File*);
extern "C" Inode* FileManager_namei(FileManager*, unsigned int mode);
extern "C" Inode* FileManager_maknode(FileManager*, unsigned int mode);
extern "C" void FileManager_writedir(FileManager*, Inode*);
extern "C" void FileManager_setcurdir(FileManager*, char* pathname);
extern "C" bool FileManager_access(FileManager*, Inode*, unsigned int mode);
extern "C" Inode* FileManager_owner(FileManager*);
extern "C" void FileManager_chmod(FileManager*);
extern "C" void FileManager_chown(FileManager*);
extern "C" void FileManager_chdir(FileManager*);
extern "C" void FileManager_link(FileManager*);
extern "C" void FileManager_unlink(FileManager*);
extern "C" void FileManager_mknod(FileManager*);

/*
 * 文件管理类(FileManager)
 * 封装了文件系统的各种系统调用在核心态下处理过程。
 */
class FileManager
{
public:
	enum DirectorySearchMode
	{
		OPEN = 0,
		CREATE = 1,
		DELETE = 2
	};

public:
	inline FileManager() {}
	inline ~FileManager() {}

	inline void Initialize() { FileManager_initialize(this); }
	inline void Open() { FileManager_open(this); }
	inline void Creat() { FileManager_creat(this); }
	inline void Open1(Inode* pInode, int mode, int trf) { FileManager_open1(this, pInode, mode, trf); }
	inline void Close() { FileManager_close(this); }
	inline void Seek() { FileManager_seek(this); }
	inline void Dup() { FileManager_dup(this); }
	inline void FStat() { FileManager_fstat(this); }
	inline void Stat() { FileManager_stat(this); }
	inline void Stat1(Inode* pInode, unsigned long statBuf) { FileManager_stat1(this, pInode, statBuf); }
	inline void Read() { FileManager_read(this); }
	inline void Write() { FileManager_write(this); }
	inline void Rdwr(enum File::FileFlags mode) { FileManager_rdwr(this, (unsigned int)mode); }
	inline void Pipe() { FileManager_pipe(this); }
	inline void ReadP(File* pFile) { FileManager_readp(this, pFile); }
	inline void WriteP(File* pFile) { FileManager_writep(this, pFile); }
	inline Inode* NameI(enum DirectorySearchMode mode) { return FileManager_namei(this, (unsigned int)mode); }
	inline Inode* MakNode(unsigned int mode) { return FileManager_maknode(this, mode); }
	inline void WriteDir(Inode* pInode) { FileManager_writedir(this, pInode); }
	inline void SetCurDir(char* pathname) { FileManager_setcurdir(this, pathname); }
	inline int Access(Inode* pInode, unsigned int mode) { return FileManager_access(this, pInode, mode); }
	inline Inode* Owner() { return FileManager_owner(this); }
	inline void ChMod() { FileManager_chmod(this); }
	inline void ChOwn() { FileManager_chown(this); }
	inline void ChDir() { FileManager_chdir(this); }
	inline void Link() { FileManager_link(this); }
	inline void UnLink() { FileManager_unlink(this); }
	inline void MkNod() { FileManager_mknod(this); }
};

class DirectoryEntry
{
public:
	static const int DIRSIZ = 28;

public:
	inline DirectoryEntry() : m_ino(0), m_name{0} {}
	inline ~DirectoryEntry() {}

public:
	int m_ino;
	char m_name[DIRSIZ];
};

#endif
