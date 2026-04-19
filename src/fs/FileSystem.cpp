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
extern "C" Buf* FileSystem_alloc(Mount*, short);
extern "C" void FileSystem_free(Mount*, short, int);
extern "C" Mount* FileSystem_get_mount(Mount*, Inode*);

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
	this->GetFS(dev);
	return FileSystem_alloc(&this->m_Mount[0], dev);
}

void FileSystem::Free(short dev, int blkno)
{
	this->GetFS(dev);
	FileSystem_free(&this->m_Mount[0], dev, blkno);
}

Mount* FileSystem::GetMount(Inode *pInode)
{
	return FileSystem_get_mount(&this->m_Mount[0], pInode);
}

bool FileSystem::BadBlock(SuperBlock *spb, short dev, int blkno)
{
	return 0;
}
