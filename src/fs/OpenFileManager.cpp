#include "OpenFileManager.h"
#include "Kernel.h"
#include "TimeInterrupt.h"
#include "Video.h"
#include "fs_defines.h"

/*==============================class OpenFileTable===================================*/
extern "C" File* OpenFileTable_f_alloc(struct open_file_table*);
extern "C" void OpenFileTable_f_close(struct open_file_table*, File*);

File* f_alloc(struct open_file_table* oft) {
	User& u = Kernel::Instance().GetUser();
        File* retval = OpenFileTable_f_alloc(oft);

        if (!retval || User_get_error()) {
                Diagnose::Write("No Free File Struct\n");
                return NULL;
        }

        return retval;
}

void f_close(struct open_file_table* oft, File* file) {
        OpenFileTable_f_close(oft, file);
}

extern "C" void f_close_bottom2(File* pFile) {
	if (!pFile->f_inode)
		return;

        pFile->f_inode->CloseI(pFile->f_flag & File::FWRITE);
        g_InodeTable.IPut(pFile->f_inode);
}

/*==============================class InodeTable===================================*/
/*  定义内存Inode表的实例 */
InodeTable g_InodeTable;

Inode* InodeTable::IGet(short dev, int inumber)
{
	Inode* pInode;
	User& u = Kernel::Instance().GetUser();

	while(true)
	{
		/* 检查指定设备dev中编号为inumber的外存Inode是否有内存拷贝 */
		int index = this->IsLoaded(dev, inumber);
		if(index >= 0)	/* 找到内存拷贝 */
		{
			pInode = &(this->m_Inode[index]);
			/* 如果该内存Inode被上锁 */
			if( pInode->i_flag & Inode::ILOCK )
			{
				/* 增设IWANT标志，然后睡眠 */
				pInode->i_flag |= Inode::IWANT;

				User_get_procp()->Sleep((unsigned long)&pInode, ProcessManager::PINOD);

				/* 回到while循环，需要重新搜索，因为该内存Inode可能已经失效 */
				continue;
			}

			/* 如果该内存Inode用于连接子文件系统，查找该Inode对应的Mount装配块 */
			if( pInode->i_flag & Inode::IMOUNT )
			{
				Mount* pMount = Kernel::Instance().GetFileSystem().GetMount(pInode);
				if(NULL == pMount)
				{
					/* 没有找到 */
					Utility::Panic("No Mount Tab...");
				}
				else
				{
					/* 将参数设为子文件系统设备号、根目录Inode编号 */
					dev = pMount->m_dev;
					inumber = FileSystem::ROOTINO;
					/* 回到while循环，以新dev，inumber值重新搜索 */
					continue;
				}
			}

			/* 
			 * 程序执行到这里表示：内存Inode高速缓存中找到相应内存Inode，
			 * 增加其引用计数，增设ILOCK标志并返回之
			 */
			pInode->i_count++;
			pInode->i_flag |= Inode::ILOCK;
			return pInode;
		}
		else	/* 没有Inode的内存拷贝，则分配一个空闲内存Inode */
		{
			pInode = this->GetFreeInode();
			/* 若内存Inode表已满，分配空闲Inode失败 */
			if(NULL == pInode)
			{
				Diagnose::Write("Inode Table Overflow !\n");
				User_get_error() = User::ENFILE;
				return NULL;
			}
			else	/* 分配空闲Inode成功，将外存Inode读入新分配的内存Inode */
			{
				/* 设置新的设备号、外存Inode编号，增加引用计数，对索引节点上锁 */
				pInode->i_dev = dev;
				pInode->i_number = inumber;
				pInode->i_flag = Inode::ILOCK;
				pInode->i_count++;
				pInode->i_lastr = -1;


			}
		}
	}
	return NULL;	/* GCC likes it! */
}

extern "C" bool compat_bread(Inode* pInode, short dev, int sector, int ino) {
        BufferManager& bm = Kernel::Instance().GetBufferManager();
        /* 将该外存Inode读入缓冲区 */
        Buf* pBuf = bm.Bread(dev, fs::INODE_SECTOR_OFF + ino / FileSystem::INODE_NUMBER_PER_SECTOR );

        /* 如果发生I/O错误 */
        if(pBuf->b_flags & Buf::B_ERROR)
        {
                /* 释放缓存 */
                bm.Brelse(pBuf);
                /* 释放占据的内存Inode */
                g_InodeTable.IPut(pInode);
                return false;
        }

        /* 将缓冲区中的外存Inode信息拷贝到新分配的内存Inode中 */
        pInode->ICopy(pBuf, ino);
        /* 释放缓存 */
        bm.Brelse(pBuf);
        return true;
}

/* close文件时会调用Iput
 *      主要做的操作：内存i节点计数 i_count--；若为0，释放内存 i节点、若有改动写回磁盘
 * 搜索文件途径的所有目录文件，搜索经过后都会Iput其内存i节点。路径名的倒数第2个路径分量一定是个
 *   目录文件，如果是在其中创建新文件、或是删除一个已有文件；再如果是在其中创建删除子目录。那么
 *   	必须将这个目录文件所对应的内存 i节点写回磁盘。
 *   	这个目录文件无论是否经历过更改，我们必须将它的i节点写回磁盘。
 * */
void InodeTable::IPut(Inode *pNode)
{
	/* 当前进程为引用该内存Inode的唯一进程，且准备释放该内存Inode */
	if(pNode->i_count == 1)
	{
		/* 
		 * 上锁，因为在整个释放过程中可能因为磁盘操作而使得该进程睡眠，
		 * 此时有可能另一个进程会对该内存Inode进行操作，这将有可能导致错误。
		 */
		pNode->i_flag |= Inode::ILOCK;

		/* 该文件已经没有目录路径指向它 */
		if(pNode->i_nlink <= 0)
		{
			/* 释放该文件占据的数据盘块 */
			pNode->ITrunc();
			pNode->i_mode = 0;
			/* 释放对应的外存Inode */
                        Kernel::Instance().GetFileSystem().IFree(pNode->i_dev, pNode->i_number);
		}

		/* 更新外存Inode信息 */
		pNode->IUpdate(Time::time);

		/* 解锁内存Inode，并且唤醒等待进程 */
		pNode->Prele();
		/* 清除内存Inode的所有标志位 */
		pNode->i_flag = 0;
		/* 这是内存inode空闲的标志之一，另一个是i_count == 0 */
		pNode->i_number = -1;
	}

	/* 减少内存Inode的引用计数，唤醒等待进程 */
	pNode->i_count--;
	pNode->Prele();
}

void InodeTable::UpdateInodeTable()
{
	for(int i = 0; i < InodeTable::NINODE; i++)
	{
		/* 
		 * 如果Inode对象没有被上锁，即当前未被其它进程使用，可以同步到外存Inode；
		 * 并且count不等于0，count == 0意味着该内存Inode未被任何打开文件引用，无需同步。
		 */
		if( (this->m_Inode[i].i_flag & Inode::ILOCK) == 0 && this->m_Inode[i].i_count != 0 )
		{
			/* 将内存Inode上锁后同步到外存Inode */
			this->m_Inode[i].i_flag |= Inode::ILOCK;
			this->m_Inode[i].IUpdate(Time::time);

			/* 对内存Inode解锁 */
			this->m_Inode[i].Prele();
		}
	}
}

int InodeTable::IsLoaded(short dev, int inumber)
{
	/* 寻找指定外存Inode的内存拷贝 */
	for(int i = 0; i < InodeTable::NINODE; i++)
	{
		if( this->m_Inode[i].i_dev == dev && this->m_Inode[i].i_number == inumber && this->m_Inode[i].i_count != 0 )
		{
			return i;	
		}
	}
	return -1;
}

Inode* InodeTable::GetFreeInode()
{
	for(int i = 0; i < InodeTable::NINODE; i++)
	{
		/* 如果该内存Inode引用计数为零，则该Inode表示空闲 */
		if(this->m_Inode[i].i_count == 0)
		{
			return &(this->m_Inode[i]);
		}
	}
	return NULL;	/* 寻找失败 */
}
