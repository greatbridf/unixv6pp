#include "FileManager.h"
#include "Kernel.h"
#include "Utility.h"
#include "TimeInterrupt.h"

#include "fs_defines.h"

/*==========================class FileManager===============================*/
FileManager::FileManager()
{
	//nothing to do here
}

FileManager::~FileManager()
{
	//nothing to do here
}

void FileManager::Initialize()
{
	this->m_FileSystem = &Kernel::Instance().GetFileSystem();
}

/*
 * 功能：打开文件
 * 效果：建立打开文件结构，内存i节点开锁 、i_count 为正数（i_count ++）
 * */
void FileManager::Open()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();

	pInode = this->NameI(FileManager::OPEN);	/* 0 = Open, not create */
	/* 没有找到相应的Inode */
	if ( NULL == pInode )
	{
		return;
	}
	this->Open1(pInode, User_get_arg()[1], 0);
}

/*
 * 功能：创建一个新的文件
 * 效果：建立打开文件结构，内存i节点开锁 、i_count 为正数（应该是 1）
 * */
void FileManager::Creat()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();
	unsigned int newACCMode = User_get_arg()[1] & (Inode::IRWXU|Inode::IRWXG|Inode::IRWXO);

	/* 搜索目录的模式为1，表示创建；若父目录不可写，出错返回 */
	pInode = this->NameI(FileManager::CREATE);
	/* 没有找到相应的Inode，或NameI出错 */
	if ( NULL == pInode )
	{
		if(User_get_error())
			return;
		/* 创建Inode */
		pInode = this->MakNode( newACCMode & (~Inode::ISVTX) );
		/* 创建失败 */
		if ( NULL == pInode )
		{
			return;
		}

		/*
		 * 如果所希望的名字不存在，使用参数trf = 2来调用open1()。
		 * 不需要进行权限检查，因为刚刚建立的文件的权限和传入参数mode
		 * 所表示的权限内容是一样的。
		 */
		this->Open1(pInode, File::FWRITE, 2);
	}
	else
	{
		/* 如果NameI()搜索到已经存在要创建的文件，则清空该文件（用算法ITrunc()）。UID没有改变
		 * 原来UNIX的设计是这样：文件看上去就像新建的文件一样。然而，新文件所有者和许可权方式没变。
		 * 也就是说creat指定的RWX比特无效。
		 * 邓蓉认为这是不合理的，应该改变。
		 * 现在的实现：creat指定的RWX比特有效 */
		this->Open1(pInode, File::FWRITE, 1);
		pInode->i_mode |= newACCMode;
	}
}

/*
* trf == 0由open调用
* trf == 1由creat调用，creat文件的时候搜索到同文件名的文件
* trf == 2由creat调用，creat文件的时候未搜索到同文件名的文件，这是文件创建时更一般的情况
* mode参数：打开文件模式，表示文件操作是 读、写还是读写
*/
void FileManager::Open1(Inode* pInode, int mode, int trf)
{
	User& u = Kernel::Instance().GetUser();

	/*
	 * 对所希望的文件已存在的情况下，即trf == 0或trf == 1进行权限检查
	 * 如果所希望的名字不存在，即trf == 2，不需要进行权限检查，因为刚建立
	 * 的文件的权限和传入的参数mode的所表示的权限内容是一样的。
	 */
	if (trf != 2)
	{
		if ( mode & File::FREAD )
		{
			/* 检查读权限 */
			this->Access(pInode, Inode::IREAD);
		}
		if ( mode & File::FWRITE )
		{
			/* 检查写权限 */
			this->Access(pInode, Inode::IWRITE);
			/* 系统调用去写目录文件是不允许的 */
			if ( (pInode->i_mode & Inode::IFMT) == Inode::IFDIR )
			{
				User_get_error() = User::EISDIR;
			}
		}
	}

	if ( User_get_error() )
	{
		InodeTable_put(pInode);
		return;
	}

	/* 在creat文件的时候搜索到同文件名的文件，释放该文件所占据的所有盘块 */
	if ( 1 == trf )
	{
		pInode->ITrunc();
	}

	/* 解锁inode!
	 * 线性目录搜索涉及大量的磁盘读写操作，期间进程会入睡。
	 * 因此，进程必须上锁操作涉及的i节点。这就是NameI中执行的IGet上锁操作。
	 * 行至此，后续不再有可能会引起进程切换的操作，可以解锁i节点。
	 */
	pInode->Prele();

	/* 分配打开文件控制块File结构 */
	File* pFile = OpenFileTable_f_alloc();
	if ( NULL == pFile )
	{
		InodeTable_put(pInode);
		return;
	}
	/* 设置打开文件方式，建立File结构和内存Inode的勾连关系 */
	pFile->f_flag = mode & (File::FREAD | File::FWRITE);
	pFile->f_inode = pInode;

	/* 特殊设备打开函数 */
	pInode->OpenI(mode & File::FWRITE);

	/* 为打开或者创建文件的各种资源都已成功分配，函数返回 */
	if ( User_get_error() == 0 )
	{
		return;
	}
	else	/* 如果出错则释放资源 */
	{
		/* 释放打开文件描述符 */
		int fd = User_get_ar0()[User::EAX];
		if(fd != -1)
		{
			OpenFiles_set_file(fd, NULL);
			/* 递减File结构和Inode的引用计数 ,File结构没有锁 f_count为0就是释放File结构了*/
			pFile->f_count--;
		}
		InodeTable_put(pInode);
	}
}

void FileManager::Close()
{
	User& u = Kernel::Instance().GetUser();
	int fd = User_get_arg()[0];

	/* 获取打开文件控制块File结构 */
	File* pFile = OpenFiles_get_file(fd);
	if ( NULL == pFile )
	{
		return;
	}

	/* 释放打开文件描述符fd，递减File结构引用计数 */
	OpenFiles_set_file(fd, NULL);
	OpenFileTable_f_close(pFile);
}

void FileManager::Seek()
{
	File* pFile;
	User& u = Kernel::Instance().GetUser();
	int fd = User_get_arg()[0];

	pFile = OpenFiles_get_file(fd);
	if ( NULL == pFile )
	{
		return;  /* 若FILE不存在，GetF有设出错码 */
	}

	/* 管道文件不允许seek */
	if ( pFile->f_flag & File::FPIPE )
	{
		User_get_error() = User::ESPIPE;
		return;
	}

	int offset = User_get_arg()[1];

	/* 如果User_get_arg()[2]在3 ~ 5之间，那么长度单位由字节变为512字节 */
	if ( User_get_arg()[2] > 2 )
	{
		offset = offset << 9;
		User_get_arg()[2] -= 3;
	}

	switch ( User_get_arg()[2] )
	{
		/* 读写位置设置为offset */
		case 0:
			pFile->f_offset = offset;
			break;
		/* 读写位置加offset(可正可负) */
		case 1:
			pFile->f_offset += offset;
			break;
		/* 读写位置调整为文件长度加offset */
		case 2:
			pFile->f_offset = pFile->f_inode->i_size + offset;
			break;
	}
}

void FileManager::Dup()
{
	File* pFile;
	User& u = Kernel::Instance().GetUser();
	int fd = User_get_arg()[0];

	pFile = OpenFiles_get_file(fd);
	if ( NULL == pFile )
	{
		return;
	}

	int newFd = OpenFiles_alloc_free_slot();
	if ( newFd < 0 )
	{
		return;
	}
	/* 至此分配新描述符newFd成功 */
	OpenFiles_set_file(newFd, pFile);
	pFile->f_count++;
}

void FileManager::FStat()
{
	File* pFile;
	User& u = Kernel::Instance().GetUser();
	int fd = User_get_arg()[0];

	pFile = OpenFiles_get_file(fd);
	if ( NULL == pFile )
	{
		return;
	}

	/* User_get_arg()[1] = pStatBuf */
	this->Stat1(pFile->f_inode, User_get_arg()[1]);
}

void FileManager::Stat()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();

	pInode = this->NameI(FileManager::OPEN);
	if ( NULL == pInode )
	{
		return;
	}
	this->Stat1(pInode, User_get_arg()[1]);
	InodeTable_put(pInode);
}

void FileManager::Stat1(Inode* pInode, unsigned long statBuf)
{
	Buf* pBuf;
	BufferManager& bufMgr = Kernel::Instance().GetBufferManager();

	pInode->IUpdate(Time::time);
	pBuf = bufMgr.Bread(pInode->i_dev, fs::INODE_SECTOR_OFF + pInode->i_number / FileSystem::INODE_NUMBER_PER_SECTOR );

	constexpr unsigned long DISK_INODE_SIZE = 64;

	/* 将p指向缓存区中编号为inumber外存Inode的偏移位置 */
	unsigned char* p = pBuf->b_addr +
		(pInode->i_number % FileSystem::INODE_NUMBER_PER_SECTOR) * DISK_INODE_SIZE;
	Utility::DWordCopy( (int *)p, (int *)statBuf, DISK_INODE_SIZE /sizeof(int) );

	bufMgr.Brelse(pBuf);
}

void FileManager::Read()
{
	/* 直接调用Rdwr()函数即可 */
	this->Rdwr(File::FREAD);
}

void FileManager::Write()
{
	/* 直接调用Rdwr()函数即可 */
	this->Rdwr(File::FWRITE);
}

void FileManager::Rdwr( enum File::FileFlags mode )
{
	File* pFile;
	User& u = Kernel::Instance().GetUser();

	/* 根据Read()/Write()的系统调用参数fd获取打开文件控制块结构 */
	pFile = OpenFiles_get_file(User_get_arg()[0]); /* fd */
	if ( NULL == pFile )
	{
		/* 不存在该打开文件，GetF已经设置过出错码，所以这里不需要再设置了 */
		/*	User_get_error() = User::EBADF;	*/
		return;
	}


	/* 读写的模式不正确 */
	if ( (pFile->f_flag & mode) == 0 )
	{
		User_get_error() = User::EACCES;
		return;
	}

	User_get_IOParam().m_Base = (unsigned char *)User_get_arg()[1];	/* 目标缓冲区首址 */
	User_get_IOParam().m_Count = User_get_arg()[2];		/* 要求读/写的字节数 */

	/* 管道读写 */
	if(pFile->f_flag & File::FPIPE)
	{
		if ( File::FREAD == mode )
		{
			this->ReadP(pFile);
		}
		else
		{
			this->WriteP(pFile);
		}
	}
	else
	/* 普通文件读写 ，或读写特殊文件。对文件实施互斥访问，互斥的粒度：每次系统调用。
	为此Inode类需要增加两个方法：NFlock()、NFrele()。
	这不是V6的设计。read、write系统调用对内存i节点上锁是为了给实施IO的进程提供一致的文件视图。*/
	{
		pFile->f_inode->NFlock();
		/* 设置文件起始读位置 */
		User_get_IOParam().m_Offset = pFile->f_offset;
		if ( File::FREAD == mode )
		{
			pFile->f_inode->ReadI();
		}
		else
		{
			pFile->f_inode->WriteI();
		}

		/* 根据读写字数，移动文件读写偏移指针 */
		pFile->f_offset += (User_get_arg()[2] - User_get_IOParam().m_Count);
		pFile->f_inode->NFrele();
	}

	/* 返回实际读写的字节数，修改存放系统调用返回值的核心栈单元 */
	User_get_ar0()[User::EAX] = User_get_arg()[2] - User_get_IOParam().m_Count;
}

void FileManager::Pipe()
{
	Inode* pInode;
	File* pFileRead;
	File* pFileWrite;
	int fd[2];
	User& u = Kernel::Instance().GetUser();

	/* 分配一个Inode用于创建管道文件 */
	pInode = this->m_FileSystem->IAlloc(DeviceManager::ROOTDEV);
	if ( NULL == pInode )
	{
		return;
	}

	/* 分配读管道的File结构 */
	pFileRead = OpenFileTable_f_alloc();
	if ( NULL == pFileRead )
	{
		InodeTable_put(pInode);
		return;
	}
	/* 读管道的打开文件描述符 */
	fd[0] = User_get_ar0()[User::EAX];

	/* 分配写管道的File结构 */
	pFileWrite = OpenFileTable_f_alloc();
	if ( NULL == pFileWrite )    /*若分配失败，擦除管道读端相关的所有打开文件结构*/
	{
		pFileRead->f_count = 0;
		OpenFiles_set_file(fd[0], NULL);
		InodeTable_put(pInode);
		return;
	}

	/* 写管道的打开文件描述符 */
	fd[1] = User_get_ar0()[User::EAX];

	/* Pipe(int* fd)的参数在User_get_arg()[0]中，将分配成功的2个fd返回给用户态程序 */
	int* pFdarr = (int *)User_get_arg()[0];
	pFdarr[0] = fd[0];
	pFdarr[1] = fd[1];

	/* 设置读、写管道File结构的属性 ，以后read、write系统调用需要这个标识*/
	pFileRead->f_flag = File::FREAD | File::FPIPE;
	pFileRead->f_inode = pInode;
	pFileWrite->f_flag = File::FWRITE | File::FPIPE;
	pFileWrite->f_inode = pInode;

	pInode->i_count = 2;
	pInode->i_flag = Inode::IACC | Inode::IUPD;
	pInode->i_mode = Inode::IALLOC;
}

void FileManager::ReadP(File *pFile)
{
	Inode* pInode = pFile->f_inode;
	User& u = Kernel::Instance().GetUser();

loop:
	/* 对管道文件上锁保证互斥 ，在现在的V6版本普通文件的读写也采取这种非常保守的上锁方式*/
	pInode->Plock();

	/* 管道中没有数据可读取 。管道文件从尾部开始写，故i_size是写指针。*/
	if ( pFile->f_offset == pInode->i_size )
	{
		if ( pFile->f_offset != 0 )
		{
			/* 读管道文件偏移量和管道文件大小重置为0 */
			pFile->f_offset = 0;
			pInode->i_size = 0;

			/* 如果置上IWRITE标志，则表示有进程正在等待写此管道，所以必须唤醒相应的进程。*/
			if ( pInode->i_mode & Inode::IWRITE )
			{
				pInode->i_mode &= (~Inode::IWRITE);
				Kernel::Instance().GetProcessManager().WakeUpAll((unsigned long)(pInode + 1));
			}
		}

		pInode->Prele(); /* 不解锁的话，写管道进程无法对管道实施操作。系统死锁 */

		/* 如果管道的读者、写者中已经有一方关闭，则返回 */
		if ( pInode->i_count < 2 )
		{
			return;
		}

		/* IREAD标志表示有进程等待读Pipe */
		pInode->i_mode |= Inode::IREAD;
		User_get_procp()->Sleep((unsigned long)(pInode + 2), ProcessManager::PPIPE);
		goto loop;
	}

	/* 管道中有有可读取的数据 */
	User_get_IOParam().m_Offset = pFile->f_offset;
	pInode->ReadI();
	pFile->f_offset = User_get_IOParam().m_Offset;
	pInode->Prele();
}

void FileManager::WriteP(File* pFile)
{
	Inode* pInode = pFile->f_inode;
	User& u = Kernel::Instance().GetUser();

	int count = User_get_IOParam().m_Count;

loop:
	pInode->Plock();

	/* 已完成所有数据写入管道，对管道unlock并返回 */
	if ( 0 == count )
	{
		pInode->Prele();
		User_get_IOParam().m_Count = 0;
		return;
	}

	/* 管道读者进程已关闭读端、用信号SIGPIPE通知应用程序 */
	if ( pInode->i_count < 2 )
	{
		pInode->Prele();
		User_get_error() = User::EPIPE;
		User_get_procp()->PSignal(User::SIGPIPE);
		return;
	}

	/* 如果已经到达管道的底，则置上同步标志，睡眠等待 */
	if ( Inode::PIPSIZ == pInode->i_size )
	{
		pInode->i_mode |= Inode::IWRITE;
		pInode->Prele();
		User_get_procp()->Sleep((unsigned long)(pInode + 1), ProcessManager::PPIPE);
		goto loop;
	}

	/* 将待写入数据尽可能多地写入管道 */
	User_get_IOParam().m_Offset = pInode->i_size;
	User_get_IOParam().m_Count = Utility::Min(count, Inode::PIPSIZ - User_get_IOParam().m_Offset);
	count -= User_get_IOParam().m_Count;
	pInode->WriteI();
	pInode->Prele();

	/* 唤醒读管道进程 */
	if ( pInode->i_mode & Inode::IREAD )
	{
		pInode->i_mode &= (~Inode::IREAD);
		Kernel::Instance().GetProcessManager().WakeUpAll((unsigned long)(pInode + 2));
	}
	goto loop;

}

extern "C" const char* Utils_get_path();
extern "C" void Utils_put_path(const char* path);

class RustPath {
public:
	RustPath() : path(Utils_get_path()) { }
	~RustPath() { Utils_put_path(this->path); }

	const char* get() const { return this->path; }

private:
	const char* path;
};

bool Inode_is_dir(Inode* pInode) {
	return (pInode->i_mode & Inode::IFMT) == Inode::IFDIR;
}

struct Buffer {
	Buf* buf;

	Buffer() noexcept : buf(nullptr) { }
	Buffer(Buf* buf) noexcept : buf(buf) { }

	Buffer(const Buffer&) = delete;
	Buffer& operator=(const Buffer&) = delete;

	Buffer(Buffer&& other) noexcept : buf(other.buf) { other.buf = nullptr; }
	Buffer& operator=(Buffer&& other) noexcept {
		if (this == &other)
			return *this;

		Buffer old((Buffer&&)*this);
		this->buf = other.buf;
		other.buf = nullptr;
		return *this;
	}

	~Buffer() noexcept {
		if (!this->buf)
			return;
		Kernel::Instance().GetBufferManager().Brelse(this->buf);
	}

	Buf* operator*() const noexcept {
		if (!this->buf)
			Utility::Panic("Buffer: null buffer");
		return this->buf;
	}

	Buf* operator->() const noexcept { return this->operator*(); }
};

Buffer Inode_read_blk(Inode* inode, unsigned long offset) {
	int phyblk = Inode_bmap(inode, offset / Inode::BLOCK_SIZE);
	return Buffer(Kernel::Instance().GetBufferManager().Bread(inode->i_dev, phyblk));
}

Inode* search_get_inode(Inode* dir, const char* name, bool create, bool remove) {
	FileManager& mgr = Kernel::Instance().GetFileManager();
	unsigned long count = dir->i_size / sizeof(DirectoryEntry);
	unsigned long offset = 0;
	unsigned long free_offset = 0;
	Inode* ret = nullptr;

	Buffer blk;
	while (count) {
		/* 已读完目录文件的当前盘块，需要读入下一目录项数据盘块 */
		if (offset % Inode::BLOCK_SIZE == 0)
			blk = Inode_read_blk(dir, offset);

		/* 读取下一目录项至User_get_dent() */
		DirectoryEntry* dentry = (DirectoryEntry*)blk->b_addr
			+ (offset % Inode::BLOCK_SIZE) / sizeof(DirectoryEntry);

		/* 如果是空闲目录项，记录该项位于目录文件中偏移量 */
		if (!dentry->m_ino) {
			if (!free_offset)
				free_offset = offset;

			/* 跳过空闲目录项，继续比较下一目录项 */
			goto next_item;
		}

		if (!strcmp(name, dentry->m_name)) {
			User_get_dent() = *dentry;
			break;
		}

	next_item:
		offset += sizeof(DirectoryEntry);
		count -= 1;
	}

	/* 没找到 */
	if (!count) {
		if (!create) {
			/* 目录项搜索完毕而没有找到匹配项，释放相关Inode资源，并推出 */
			User_get_error() = User::ENOENT;
			goto out;
		}

		/* 如果是创建新文件，判断该目录是否可写 */
		if (mgr.Access(dir, Inode::IWRITE)) {
			User_get_error() = User::EACCES;
			goto out;
		}

		/* 将父目录Inode指针保存起来，以后写目录项WriteDir()函数会用到 */
		User_get_pdir() = InodeTable_get(dir->i_dev, dir->i_number);

		/* 如果前面没有空闲的，当前（最后）就是空闲的 */
		/* 问题：为何另一分支没有置IUPD标志？ 这是因为文件的长度没有变呀 */
		if (!free_offset) {
			free_offset = offset;
			dir->i_flag |= Inode::IUPD;
		}

		/* 将空闲目录项偏移量存入u区中，写目录项WriteDir()会用到 */
		offset = free_offset;

		/* 找到可以写入的空闲目录项位置，NameI()函数返回 */
		goto out;
	}

	/* 如果是删除操作，则返回父目录Inode，而要删除文件的Inode号在User_get_dent().m_ino中 */
	if (remove) {
		/* 如果对父目录没有写的权限 */
		if (mgr.Access(dir, Inode::IWRITE))
			User_get_error() = User::EACCES;
		goto out;
	}


	/* 匹配目录项成功，根据匹配成功的目录项m_ino字段获取相应下一级目录或文件的Inode。 */
	ret = InodeTable_get(dir->i_dev, User_get_dent().m_ino);

out:
	User_get_IOParam().m_Offset = offset;
	User_get_IOParam().m_Count = count;
	return ret;
}

/* 返回NULL表示目录搜索失败，否则是根指针，指向文件的内存打开i节点 ，上锁的内存i节点  */
Inode* FileManager::NameI(enum DirectorySearchMode mode )
{
	Inode* pInode;
	Buf* pBuf;
	char curchar;
	char* pChar;
	int freeEntryOffset;	/* 以创建文件模式搜索目录时，记录空闲目录项的偏移量 */
	User& u = Kernel::Instance().GetUser();
	BufferManager& bufMgr = Kernel::Instance().GetBufferManager();
	RustPath path;
	const char* cur = path.get();

	/* 如果该路径是'/'开头的，从根目录开始搜索，否则从进程当前工作目录开始搜索。 */
	if (*cur == '/')
		pInode = this->rootDirInode;
	else
		pInode = User_get_cdir();

	/* 检查该Inode是否正在被使用，以及保证在整个目录搜索过程中该Inode不被释放 */
	pInode = InodeTable_get(pInode->i_dev, pInode->i_number);

	/* 允许出现////a//b 这种路径 这种路径等价于/a/b */
	while (*cur == '/')
		cur++;

	while (User_get_error() == User::NOERROR && *cur) {
		const char* comp = cur;

		if (!Inode_is_dir(pInode)) {
			User_get_error() = User::ENOTDIR;
			goto err_put;
		}

		if (this->Access(pInode, Inode::IEXEC)) {
			User_get_error() = User::EACCES;
			goto err_put;
		}

		while (*cur && *cur != '/')
			cur++;

		unsigned long name_len = cur - comp;
		if (name_len > DirectoryEntry::DIRSIZ - 1)
			Utility::Panic("Name too long");

		char* dbuf = User_get_dbuf();
		char* dbuf_end = User_get_dbuf() + DirectoryEntry::DIRSIZ;
		while (comp < cur)
			*dbuf++ = *comp++;
		while (dbuf < dbuf_end)
			*dbuf++ = 0;

		while (*cur && *cur == '/')
			cur++;

		Inode* next = search_get_inode(pInode, User_get_dbuf(),
			mode == CREATE && !*cur, mode == DELETE && !*cur);

		if (!next)
			goto err_put;

		InodeTable_put(pInode);
		pInode = next;
	}

	return pInode;

err_put:
	InodeTable_put(pInode);
	return nullptr;
}

/* 由creat调用。
 * 为新创建的文件写新的i节点和新的目录项
 * 返回的pInode是上了锁的内存i节点，其中的i_count是 1。
 *
 * 在程序的最后会调用 WriteDir，在这里把属于自己的目录项写进父目录，修改父目录文件的i节点 、将其写回磁盘。
 *
 */
Inode* FileManager::MakNode( unsigned int mode )
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();

	/* 分配一个空闲DiskInode，里面内容已全部清空 */
	pInode = this->m_FileSystem->IAlloc(User_get_pdir()->i_dev);
	if( NULL ==	pInode )
	{
		return NULL;
	}

	pInode->i_flag |= (Inode::IACC | Inode::IUPD);
	pInode->i_mode = mode | Inode::IALLOC;
	pInode->i_nlink = 1;
	pInode->i_uid = User_get_uid();
	pInode->i_gid = User_get_gid();
	/* 将目录项写入User_get_dent()，随后写入目录文件 */
	this->WriteDir(pInode);
	return pInode;
}

void FileManager::WriteDir( Inode* pInode )
{
	User& u = Kernel::Instance().GetUser();

	/* 设置目录项中Inode编号部分 */
	User_get_dent().m_ino = pInode->i_number;

	/* 设置目录项中pathname分量部分 */
	for ( int i = 0; i < DirectoryEntry::DIRSIZ; i++ )
	{
		User_get_dent().m_name[i] = User_get_dbuf()[i];
	}

	User_get_IOParam().m_Count = DirectoryEntry::DIRSIZ + 4;
	User_get_IOParam().m_Base = (unsigned char *)&User_get_dent();

	/* 将目录项写入父目录文件 */
	User_get_pdir()->WriteI();
	InodeTable_put(User_get_pdir());
}

void FileManager::SetCurDir(char* pathname)
{
	User& u = Kernel::Instance().GetUser();

	/* 路径不是从根目录'/'开始，则在现有User_get_curdir()后面加上当前路径分量 */
	if ( pathname[0] != '/' )
	{
		int length = strlen(User_get_curdir());
		if ( User_get_curdir()[length - 1] != '/' )
		{
			User_get_curdir()[length] = '/';
			length++;
		}
		strcpy(User_get_curdir() + length, pathname);
	}
	else	/* 如果是从根目录'/'开始，则取代原有工作目录 */
	{
		strcpy(User_get_curdir(), pathname);
	}
}

/*
 * 返回值是0，表示拥有打开文件的权限；1表示没有所需的访问权限。文件未能打开的原因记录在User_get_error()变量中。
 */
int FileManager::Access( Inode* pInode, unsigned int mode )
{
	User& u = Kernel::Instance().GetUser();

	/* 对于写的权限，必须检查该文件系统是否是只读的 */
	if ( Inode::IWRITE == mode )
	{
		if( this->m_FileSystem->IsReadOnly(pInode->i_dev) )
		{
			User_get_error() = User::EROFS;
			return 1;
		}
	}
	/*
	 * 对于超级用户，读写任何文件都是允许的
	 * 而要执行某文件时，必须在i_mode有可执行标志
	 */
	if ( User_get_uid() == 0 )
	{
		if ( Inode::IEXEC == mode && ( pInode->i_mode & (Inode::IEXEC | (Inode::IEXEC >> 3) | (Inode::IEXEC >> 6)) ) == 0 )
		{
			User_get_error() = User::EACCES;
			return 1;
		}
		return 0;	/* Permission Check Succeed! */
	}
	if ( User_get_uid() != pInode->i_uid )
	{
		mode = mode >> 3;
		if ( User_get_gid() != pInode->i_gid )
		{
			mode = mode >> 3;
		}
	}
	if ( (pInode->i_mode & mode) != 0 )
	{
		return 0;
	}

	User_get_error() = User::EACCES;
	return 1;
}

Inode* FileManager::Owner()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();

	if ( (pInode = this->NameI(FileManager::OPEN)) == NULL )
	{
		return NULL;
	}

	if ( User_get_uid() == pInode->i_uid || Userspace_is_root() )
	{
		return pInode;
	}

	InodeTable_put(pInode);
	return NULL;
}

void FileManager::ChMod()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();
	unsigned int mode = User_get_arg()[1];

	if ( (pInode = this->Owner()) == NULL )
	{
		return;
	}
	/* clear i_mode字段中的ISGID, ISUID, ISTVX以及rwxrwxrwx这12比特 */
	pInode->i_mode &= (~0xFFF);
	/* 根据系统调用的参数重新设置i_mode字段 */
	pInode->i_mode |= (mode & 0xFFF);
	pInode->i_flag |= Inode::IUPD;

	InodeTable_put(pInode);
	return;
}

void FileManager::ChOwn()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();
	short uid = User_get_arg()[1];
	short gid = User_get_arg()[2];

	/* 不是超级用户或者不是文件主则返回 */
	if ( !Userspace_is_root() || (pInode = this->Owner()) == NULL )
	{
		return;
	}
	pInode->i_uid = uid;
	pInode->i_gid = gid;
	pInode->i_flag |= Inode::IUPD;

	InodeTable_put(pInode);
}

void FileManager::ChDir()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();

	pInode = this->NameI(FileManager::OPEN);
	if ( NULL == pInode )
	{
		return;
	}
	/* 搜索到的文件不是目录文件 */
	if ( (pInode->i_mode & Inode::IFMT) != Inode::IFDIR )
	{
		User_get_error() = User::ENOTDIR;
		InodeTable_put(pInode);
		return;
	}
	if ( this->Access(pInode, Inode::IEXEC) )
	{
		InodeTable_put(pInode);
		return;
	}
	InodeTable_put(User_get_cdir());
	User_get_cdir() = pInode;
	pInode->Prele();

	this->SetCurDir((char *)User_get_arg()[0] /* pathname */);
}

void FileManager::Link()
{
	Inode* pInode;
	Inode* pNewInode;
	User& u = Kernel::Instance().GetUser();

	pInode = this->NameI(FileManager::OPEN);
	/* 打卡文件失败 */
	if ( NULL == pInode )
	{
		return;
	}
	/* 链接的数量已经最大 */
	if ( pInode->i_nlink >= 255 )
	{
		User_get_error() = User::EMLINK;
		/* 出错，释放资源并退出 */
		InodeTable_put(pInode);
		return;
	}
	/* 对目录文件的链接只能由超级用户进行 */
	if ( (pInode->i_mode & Inode::IFMT) == Inode::IFDIR && !Userspace_is_root() )
	{
		/* 出错，释放资源并退出 */
		InodeTable_put(pInode);
		return;
	}

	/* 解锁现存文件Inode,以避免在以下搜索新文件时产生死锁 */
	pInode->i_flag &= (~Inode::ILOCK);
	/* 指向要创建的新路径newPathname */
	User_get_dirp() = (char *)User_get_arg()[1];
	pNewInode = this->NameI(FileManager::CREATE);
	/* 如果文件已存在 */
	if ( NULL != pNewInode )
	{
		User_get_error() = User::EEXIST;
		InodeTable_put(pNewInode);
	}
	if ( User::NOERROR != User_get_error() )
	{
		/* 出错，释放资源并退出 */
		InodeTable_put(pInode);
		return;
	}
	/* 检查目录与该文件是否在同一个设备上 */
	if ( User_get_pdir()->i_dev != pInode->i_dev )
	{
		InodeTable_put(User_get_pdir());
		User_get_error() = User::EXDEV;
		/* 出错，释放资源并退出 */
		InodeTable_put(pInode);
		return;
	}

	this->WriteDir(pInode);
	pInode->i_nlink++;
	pInode->i_flag |= Inode::IUPD;
	InodeTable_put(pInode);
}

void FileManager::UnLink()
{
	Inode* pInode;
	Inode* pDeleteInode;
	User& u = Kernel::Instance().GetUser();

	pDeleteInode = this->NameI(FileManager::DELETE);
	if ( NULL == pDeleteInode )
	{
		return;
	}
	pDeleteInode->Prele();

	pInode = InodeTable_get(pDeleteInode->i_dev, User_get_dent().m_ino);
	if ( NULL == pInode )
	{
		Utility::Panic("unlink -- iget");
	}
	/* 只有root可以unlink目录文件 */
	if ( (pInode->i_mode & Inode::IFMT) == Inode::IFDIR && !Userspace_is_root() )
	{
		InodeTable_put(pDeleteInode);
		InodeTable_put(pInode);
		return;
	}
	/* 写入清零后的目录项 */
	User_get_IOParam().m_Offset -= (DirectoryEntry::DIRSIZ + 4);
	User_get_IOParam().m_Base = (unsigned char *)&User_get_dent();
	User_get_IOParam().m_Count = DirectoryEntry::DIRSIZ + 4;

	User_get_dent().m_ino = 0;
	pDeleteInode->WriteI();

	/* 修改inode项 */
	pInode->i_nlink--;
	pInode->i_flag |= Inode::IUPD;

	InodeTable_put(pDeleteInode);
	InodeTable_put(pInode);
}

void FileManager::MkNod()
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();

	/* 检查uid是否是root，该系统调用只有uid==root时才可被调用 */
	if ( Userspace_is_root() )
	{
		pInode = this->NameI(FileManager::CREATE);
		/* 要创建的文件已经存在,这里并不能去覆盖此文件 */
		if ( pInode != NULL )
		{
			User_get_error() = User::EEXIST;
			InodeTable_put(pInode);
			return;
		}
	}
	else
	{
		/* 非root用户执行mknod()系统调用返回User::EPERM */
		User_get_error() = User::EPERM;
		return;
	}
	/* 没有通过SUser()的检查 */
	if ( User::NOERROR != User_get_error() )
	{
		return;	/* 没有需要释放的资源，直接退出 */
	}
	pInode = this->MakNode(User_get_arg()[1]);
	if ( NULL == pInode )
	{
		return;
	}
	/* 所建立是设备文件 */
	if ( (pInode->i_mode & (Inode::IFBLK | Inode::IFCHR)) != 0 )
                Inode_set_dev(pInode, User_get_arg()[2]);
	InodeTable_put(pInode);
}
/*==========================class DirectoryEntry===============================*/
DirectoryEntry::DirectoryEntry()
{
	this->m_ino = 0;
	this->m_name[0] = '\0';
}

DirectoryEntry::~DirectoryEntry()
{
	//nothing to do here
}

