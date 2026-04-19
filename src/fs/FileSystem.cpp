#include "FileSystem.h"
#include "Utility.h"
#include "New.h"
#include "Kernel.h"
#include "OpenFileManager.h"
#include "TimeInterrupt.h"
#include "Video.h"
#include "fs_defines.h"

extern "C" bool FileSystem_load_super_block(Mount*, SuperBlock*);
extern "C" SuperBlock* FileSystem_get_fs(Mount*, short);
extern "C" void FileSystem_update(Mount*, int*);
extern "C" Inode* FileSystem_i_alloc(Mount*, short);
extern "C" void FileSystem_i_free(Mount*, short, int);

/*==============================class SuperBlock===================================*/
/* 系统全局超级块SuperBlock对象 */
SuperBlock g_spb;

SuperBlock::SuperBlock()
{
	//nothing to do here
}

SuperBlock::~SuperBlock()
{
	//nothing to do here
}

/*==============================class Mount===================================*/
Mount::Mount()
{
	this->m_dev = -1;
	this->m_spb = NULL;
	this->m_inodep = NULL;
}

Mount::~Mount()
{
	this->m_dev = -1;
	this->m_inodep = NULL;
	//释放内存SuperBlock副本
	if(!this->m_spb)
                return;

        delete this->m_spb;
        this->m_spb = NULL;
}

/*==============================class FileSystem===================================*/
FileSystem::FileSystem()
{
	//nothing to do here
}

FileSystem::~FileSystem()
{
	//nothing to do here
}

void FileSystem::Initialize()
{
	this->m_BufferManager = &Kernel::Instance().GetBufferManager();
	this->updlock = 0;
}

void FileSystem::LoadSuperBlock()
{
	if (!FileSystem_load_super_block(&this->m_Mount[0], &g_spb))
                Utility::Panic("Load SuperBlock Error....!\n");
}

SuperBlock* FileSystem::GetFS(short dev)
{
	SuperBlock* sb = FileSystem_get_fs(&this->m_Mount[0], dev);
	if(sb != NULL)
		return sb;

	Utility::Panic("No File System!");
	return NULL;
}

void FileSystem::Update()
{
	FileSystem_update(&this->m_Mount[0], &this->updlock);
}

Inode* FileSystem::IAlloc(short dev)
{
	this->GetFS(dev);
	return FileSystem_i_alloc(&this->m_Mount[0], dev);
}

void FileSystem::IFree(short dev, int number)
{
	this->GetFS(dev);
	FileSystem_i_free(&this->m_Mount[0], dev, number);
}

Buf* FileSystem::Alloc(short dev)
{
	int blkno;	/* 分配到的空闲磁盘块编号 */
	SuperBlock* sb;
	Buf* pBuf;
	User& u = Kernel::Instance().GetUser();

	/* 获取SuperBlock对象的内存副本 */
	sb = this->GetFS(dev);

	/*
	 * 如果空闲磁盘块索引表正在被上锁，表明有其它进程
	 * 正在操作空闲磁盘块索引表，因而对其上锁。这通常
	 * 是由于其余进程调用Free()或Alloc()造成的。
	 */
	while(sb->s_flock)
	{
		/* 进入睡眠直到获得该锁才继续 */
		User_get_procp()->Sleep((unsigned long)&sb->s_flock, ProcessManager::PINOD);
	}

	/* 从索引表“栈顶”获取空闲磁盘块编号 */
	blkno = sb->s_free[--sb->s_nfree];

	/*
	 * 若获取磁盘块编号为零，则表示已分配尽所有的空闲磁盘块。
	 * 或者分配到的空闲磁盘块编号不属于数据盘块区域中(由BadBlock()检查)，
	 * 都意味着分配空闲磁盘块操作失败。
	 */
	if(0 == blkno )
	{
		sb->s_nfree = 0;
		Diagnose::Write("No Space On %d !\n", dev);
		User_get_error() = User::ENOSPC;
		return NULL;
	}
	if( this->BadBlock(sb, dev, blkno) )
	{
		return NULL;
	}

	/*
	 * 栈已空，新分配到空闲磁盘块中记录了下一组空闲磁盘块的编号,
	 * 将下一组空闲磁盘块的编号读入SuperBlock的空闲磁盘块索引表s_free[100]中。
	 */
	if(sb->s_nfree <= 0)
	{
		/*
		 * 此处加锁，因为以下要进行读盘操作，有可能发生进程切换，
		 * 新上台的进程可能对SuperBlock的空闲盘块索引表访问，会导致不一致性。
		 */
		sb->s_flock++;

		/* 读入该空闲磁盘块 */
		pBuf = this->m_BufferManager->Bread(dev, blkno);

		/* 从该磁盘块的0字节开始记录，共占据4(s_nfree)+400(s_free[100])个字节 */
		int* p = (int *)pBuf->b_addr;

		/* 首先读出空闲盘块数s_nfree */
		sb->s_nfree = *p++;

		/* 读取缓存中后续位置的数据，写入到SuperBlock空闲盘块索引表s_free[100]中 */
		Utility::DWordCopy(p, sb->s_free, 100);

		/* 缓存使用完毕，释放以便被其它进程使用 */
		this->m_BufferManager->Brelse(pBuf);

		/* 解除对空闲磁盘块索引表的锁，唤醒因为等待锁而睡眠的进程 */
		sb->s_flock = 0;
		Kernel::Instance().GetProcessManager().WakeUpAll((unsigned long)&sb->s_flock);
	}

	/* 普通情况下成功分配到一空闲磁盘块 */
	pBuf = this->m_BufferManager->GetBlk(dev, blkno);	/* 为该磁盘块申请缓存 */
	this->m_BufferManager->ClrBuf(pBuf);	/* 清空缓存中的数据 */
	sb->s_fmod = 1;	/* 设置SuperBlock被修改标志 */

	return pBuf;
}

void FileSystem::Free(short dev, int blkno)
{
	SuperBlock* sb;
	Buf* pBuf;
	User& u = Kernel::Instance().GetUser();

	sb = this->GetFS(dev);

	/*
	 * 尽早设置SuperBlock被修改标志，以防止在释放
	 * 磁盘块Free()执行过程中，对SuperBlock内存副本
	 * 的修改仅进行了一半，就更新到磁盘SuperBlock去
	 */
	sb->s_fmod = 1;

	/* 如果空闲磁盘块索引表被上锁，则睡眠等待解锁 */
	while(sb->s_flock)
	{
		User_get_procp()->Sleep((unsigned long)&sb->s_flock, ProcessManager::PINOD);
	}

	/* 检查释放磁盘块的合法性 */
	if(this->BadBlock(sb, dev, blkno))
	{
		return;
	}

	/*
	 * 如果先前系统中已经没有空闲盘块，
	 * 现在释放的是系统中第1块空闲盘块
	 */
	if(sb->s_nfree <= 0)
	{
		sb->s_nfree = 1;
		sb->s_free[0] = 0;	/* 使用0标记空闲盘块链结束标志 */
	}

	/* SuperBlock中直接管理空闲磁盘块号的栈已满 */
	if(sb->s_nfree >= 100)
	{
		sb->s_flock++;

		/*
		 * 使用当前Free()函数正要释放的磁盘块，存放前一组100个空闲
		 * 磁盘块的索引表
		 */
		pBuf = this->m_BufferManager->GetBlk(dev, blkno);	/* 为当前正要释放的磁盘块分配缓存 */

		/* 从该磁盘块的0字节开始记录，共占据4(s_nfree)+400(s_free[100])个字节 */
		int* p = (int *)pBuf->b_addr;

		/* 首先写入空闲盘块数，除了第一组为99块，后续每组都是100块 */
		*p++ = sb->s_nfree;
		/* 将SuperBlock的空闲盘块索引表s_free[100]写入缓存中后续位置 */
		Utility::DWordCopy(sb->s_free, p, 100);

		sb->s_nfree = 0;
		/* 将存放空闲盘块索引表的“当前释放盘块”写入磁盘，即实现了空闲盘块记录空闲盘块号的目标 */
		this->m_BufferManager->Bwrite(pBuf);

		sb->s_flock = 0;
		Kernel::Instance().GetProcessManager().WakeUpAll((unsigned long)&sb->s_flock);
	}
	sb->s_free[sb->s_nfree++] = blkno;	/* SuperBlock中记录下当前释放盘块号 */
	sb->s_fmod = 1;
}

Mount* FileSystem::GetMount(Inode *pInode)
{
	/* 遍历系统的装配块表 */
	for(int i = 0; i <= FileSystem::NMOUNT; i++)
	{
		Mount* pMount = &(this->m_Mount[i]);

		/* 找到内存Inode对应的Mount装配块 */
		if(pMount->m_inodep == pInode)
		{
			return pMount;
		}
	}
	return NULL;	/* 查找失败 */
}

bool FileSystem::BadBlock(SuperBlock *spb, short dev, int blkno)
{
	return 0;
}
