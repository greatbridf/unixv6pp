#include "INode.h"
#include "Utility.h"
#include "DeviceManager.h"
#include "Kernel.h"
#include "fs_defines.h"

/*==============================class Inode===================================*/
/*	预读块的块号，对普通文件这是预读块所在的物理块号。对硬盘而言，这是当前物理块（扇区）的下一个物理块（扇区）*/
int Inode::rablock = 0;

extern "C" Buf* compat_filesys_alloc(short dev) {
        return Kernel::Instance().GetFileSystem().Alloc(dev);
}

int Inode::Bmap(int lbn)
{
	Buf* pFirstBuf;
	Buf* pSecondBuf;
	int phyBlkno;	/* 转换后的物理盘块号 */
	int* iTable;	/* 用于访问索引盘块中一次间接、两次间接索引表 */
	int index;
	User& u = Kernel::Instance().GetUser();
	BufferManager& bufMgr = Kernel::Instance().GetBufferManager();
	FileSystem& fileSys = Kernel::Instance().GetFileSystem();

	/*
	 * Unix V6++的文件索引结构：(小型、大型和巨型文件)
	 * (1) i_addr[0] - i_addr[5]为直接索引表，文件长度范围是0 - 6个盘块；
	 *
	 * (2) i_addr[6] - i_addr[7]存放一次间接索引表所在磁盘块号，每磁盘块
	 * 上存放128个文件数据盘块号，此类文件长度范围是7 - (128 * 2 + 6)个盘块；
	 *
	 * (3) i_addr[8] - i_addr[9]存放二次间接索引表所在磁盘块号，每个二次间接
	 * 索引表记录128个一次间接索引表所在磁盘块号，此类文件长度范围是
	 * (128 * 2 + 6 ) < size <= (128 * 128 * 2 + 128 * 2 + 6)
	 */

	if(lbn >= Inode::HUGE_FILE_BLOCK)
	{
		User_get_error() = User::EFBIG;
		return 0;
	}

	if(lbn < 6)		/* 如果是小型文件，从基本索引表i_addr[0-5]中获得物理盘块号即可 */
	{
		phyBlkno = this->i_addr[lbn];

		/*
		 * 如果该逻辑块号还没有相应的物理盘块号与之对应，则分配一个物理块。
		 * 这通常发生在对文件的写入，当写入位置超出文件大小，即对当前
		 * 文件进行扩充写入，就需要分配额外的磁盘块，并为之建立逻辑块号
		 * 与物理盘块号之间的映射。
		 */
		if( phyBlkno == 0 && (pFirstBuf = fileSys.Alloc(this->i_dev)) != NULL )
		{
			/*
			 * 因为后面很可能马上还要用到此处新分配的数据块，所以不急于立刻输出到
			 * 磁盘上；而是将缓存标记为延迟写方式，这样可以减少系统的I/O操作。
			 */
			bufMgr.Bdwrite(pFirstBuf);
			phyBlkno = pFirstBuf->b_blkno;
			/* 将逻辑块号lbn映射到物理盘块号phyBlkno */
			this->i_addr[lbn] = phyBlkno;
			this->i_flag |= Inode::IUPD;
		}
		/* 找到预读块对应的物理盘块号 */
		Inode::rablock = 0;
		if(lbn <= 4)
		{
			/*
			 * i_addr[0] - i_addr[5]为直接索引表。如果预读块对应物理块号可以从
			 * 直接索引表中获得，则记录在Inode::rablock中。如果需要额外的I/O开销
			 * 读入间接索引块，就显得不太值得了。漂亮！
			 */
			Inode::rablock = this->i_addr[lbn + 1];
		}

		return phyBlkno;
	}
	else	/* lbn >= 6 大型、巨型文件 */
	{
		/* 计算逻辑块号lbn对应i_addr[]中的索引 */

		if(lbn < Inode::LARGE_FILE_BLOCK)	/* 大型文件: 长度介于7 - (128 * 2 + 6)个盘块之间 */
		{
			index = (lbn - Inode::SMALL_FILE_BLOCK) / Inode::ADDRESS_PER_INDEX_BLOCK + 6;
		}
		else	/* 巨型文件: 长度介于263 - (128 * 128 * 2 + 128 * 2 + 6)个盘块之间 */
		{
			index = (lbn - Inode::LARGE_FILE_BLOCK) / (Inode::ADDRESS_PER_INDEX_BLOCK * Inode::ADDRESS_PER_INDEX_BLOCK) + 8;
		}

		phyBlkno = this->i_addr[index];
		/* 若该项为零，则表示不存在相应的间接索引表块 */
		if( 0 == phyBlkno )
		{
			this->i_flag |= Inode::IUPD;
			/* 分配一空闲盘块存放间接索引表 */
			if( (pFirstBuf = fileSys.Alloc(this->i_dev)) == NULL )
			{
				return 0;	/* 分配失败 */
			}
			/* i_addr[index]中记录间接索引表的物理盘块号 */
			this->i_addr[index] = pFirstBuf->b_blkno;
		}
		else
		{
			/* 读出存储间接索引表的字符块 */
			pFirstBuf = bufMgr.Bread(this->i_dev, phyBlkno);
		}
		/* 获取缓冲区首址 */
		iTable = (int *)pFirstBuf->b_addr;

		if(index >= 8)	/* ASSERT: 8 <= index <= 9 */
		{
			/*
			 * 对于巨型文件的情况，pFirstBuf中是二次间接索引表，
			 * 还需根据逻辑块号，经由二次间接索引表找到一次间接索引表
			 */
			index = ( (lbn - Inode::LARGE_FILE_BLOCK) / Inode::ADDRESS_PER_INDEX_BLOCK ) % Inode::ADDRESS_PER_INDEX_BLOCK;

			/* iTable指向缓存中的二次间接索引表。该项为零，不存在一次间接索引表 */
			phyBlkno = iTable[index];
			if( 0 == phyBlkno )
			{
				if( (pSecondBuf = fileSys.Alloc(this->i_dev)) == NULL)
				{
					/* 分配一次间接索引表磁盘块失败，释放缓存中的二次间接索引表，然后返回 */
					bufMgr.Brelse(pFirstBuf);
					return 0;
				}
				/* 将新分配的一次间接索引表磁盘块号，记入二次间接索引表相应项 */
				iTable[index] = pSecondBuf->b_blkno;
				/* 将更改后的二次间接索引表延迟写方式输出到磁盘 */
				bufMgr.Bdwrite(pFirstBuf);
			}
			else
			{
				/* 释放二次间接索引表占用的缓存，并读入一次间接索引表 */
				bufMgr.Brelse(pFirstBuf);
				pSecondBuf = bufMgr.Bread(this->i_dev, phyBlkno);
			}

			pFirstBuf = pSecondBuf;
			/* 令iTable指向一次间接索引表 */
			iTable = (int *)pSecondBuf->b_addr;
		}

		/* 计算逻辑块号lbn最终位于一次间接索引表中的表项序号index */

		if( lbn < Inode::LARGE_FILE_BLOCK )
		{
			index = (lbn - Inode::SMALL_FILE_BLOCK) % Inode::ADDRESS_PER_INDEX_BLOCK;
		}
		else
		{
			index = (lbn - Inode::LARGE_FILE_BLOCK) % Inode::ADDRESS_PER_INDEX_BLOCK;
		}

		if( (phyBlkno = iTable[index]) == 0 && (pSecondBuf = fileSys.Alloc(this->i_dev)) != NULL)
		{
			/* 将分配到的文件数据盘块号登记在一次间接索引表中 */
			phyBlkno = pSecondBuf->b_blkno;
			iTable[index] = phyBlkno;
			/* 将数据盘块、更改后的一次间接索引表用延迟写方式输出到磁盘 */
			bufMgr.Bdwrite(pSecondBuf);
			bufMgr.Bdwrite(pFirstBuf);
		}
		else
		{
			/* 释放一次间接索引表占用缓存 */
			bufMgr.Brelse(pFirstBuf);
		}
		/* 找到预读块对应的物理盘块号，如果获取预读块号需要额外的一次for间接索引块的IO，不合算，放弃 */
		Inode::rablock = 0;
		if( index + 1 < Inode::ADDRESS_PER_INDEX_BLOCK)
		{
			Inode::rablock = iTable[index + 1];
		}
		return phyBlkno;
	}
}

void Inode::OpenI(int mode)
{
	short dev;
	DeviceManager& devMgr = Kernel::Instance().GetDeviceManager();
	User& u = Kernel::Instance().GetUser();

	/*
	 * 对于特殊块设备、字符设备文件，i_addr[]不再是
	 * 磁盘块号索引表，addr[0]中存放了设备号dev
	 */
	dev = this->i_addr[0];

	/* 提取主设备号 */
	short major = Utility::GetMajor(dev);

	switch( this->i_mode & Inode::IFMT)
	{
	case Inode::IFCHR:	/* 字符设备特殊类型文件 */
		if (major >= devMgr.GetNChrDev())
		{
			User_get_error() = User::ENXIO;   /* no such device */
			return;
		}
		devMgr.GetCharDevice(major).Open(dev,mode);
		break;

	case Inode::IFBLK:	/* 块设备特殊类型文件 */
		/* 检查设备号是否超出系统中块设备数量 */
		if(major >= devMgr.GetNBlkDev())
		{
			User_get_error() = User::ENXIO;    /* no such device */
			return;
		}
		/* 根据主设备号获取对应的块设备BlockDevice对象引用 */
		BlockDevice& bdev = devMgr.GetBlockDevice(major);
		/* 调用该设备的特定初始化逻辑 */
		bdev.Open(dev, mode);
		break;
	}

	return;
}

void Inode::CloseI(int mode)
{
	short dev;
	DeviceManager& devMgr = Kernel::Instance().GetDeviceManager();

	/* addr[0]中存放了设备号dev */
	dev = this->i_addr[0];

	short major = Utility::GetMajor(dev);

	/* 不再使用该文件,关闭特殊文件 */
	if(this->i_count <= 1)
	{
		switch( this->i_mode & Inode::IFMT)
		{
		case Inode::IFCHR:
			devMgr.GetCharDevice(major).Close(dev, mode);
			break;

		case Inode::IFBLK:
			/* 根据主设备号获取对应的块设备BlockDevice对象引用 */
			BlockDevice& bdev = devMgr.GetBlockDevice(major);
			/* 调用该设备的特定关闭逻辑 */
			bdev.Close(dev, mode);
			break;
		}
	}
}

void Inode::IUpdate(int time)
{
	Buf* pBuf;
	DiskInode dInode;
	FileSystem& filesys = Kernel::Instance().GetFileSystem();
	BufferManager& bufMgr = Kernel::Instance().GetBufferManager();

	/* 当IUPD和IACC标志之一被设置，才需要更新相应DiskInode
	 * 目录搜索，不会设置所途径的目录文件的IACC和IUPD标志 */
	if( (this->i_flag & (Inode::IUPD | Inode::IACC))!= 0 )
	{
		if( filesys.GetFS(this->i_dev)->s_ronly != 0 )
		{
			/* 如果该文件系统只读 */
			return;
		}

		/* 邓蓉的注释：在缓存池中找到包含本i节点（this->i_number）的缓存块
		 * 这是一个上锁的缓存块，本段代码中的Bwrite()在将缓存块写回磁盘后会释放该缓存块。
		 * 将该存放该DiskInode的字符块读入缓冲区 */
		pBuf = bufMgr.Bread(this->i_dev, fs::INODE_SECTOR_OFF + this->i_number / FileSystem::INODE_NUMBER_PER_SECTOR);

		/* 将内存Inode副本中的信息复制到dInode中，然后将dInode覆盖缓存中旧的外存Inode */
		dInode.d_mode = this->i_mode;
		dInode.d_nlink = this->i_nlink;
		dInode.d_uid = this->i_uid;
		dInode.d_gid = this->i_gid;
		dInode.d_size = this->i_size;
		for (int i = 0; i < 10; i++)
		{
			dInode.d_addr[i] = this->i_addr[i];
		}
		if (this->i_flag & Inode::IACC)
		{
			/* 更新最后访问时间 */
			dInode.d_atime = time;
		}
		if (this->i_flag & Inode::IUPD)
		{
			/* 更新最后访问时间 */
			dInode.d_mtime = time;
		}

		/* 将p指向缓存区中旧外存Inode的偏移位置 */
		unsigned char* p = pBuf->b_addr + (this->i_number % FileSystem::INODE_NUMBER_PER_SECTOR) * sizeof(DiskInode);
		DiskInode* pNode = &dInode;

		/* 用dInode中的新数据覆盖缓存中的旧外存Inode */
		Utility::DWordCopy( (int *)pNode, (int *)p, sizeof(DiskInode)/sizeof(int) );

		/* 将缓存写回至磁盘，达到更新旧外存Inode的目的 */
		bufMgr.Bwrite(pBuf);
	}
}

void Inode::ITrunc()
{
	/* 经由磁盘高速缓存读取存放一次间接、两次间接索引表的磁盘块 */
	BufferManager& bm = Kernel::Instance().GetBufferManager();
	/* 获取g_FileSystem对象的引用，执行释放磁盘块的操作 */
	FileSystem& filesys = Kernel::Instance().GetFileSystem();

	/* 如果是字符设备或者块设备则退出 */
	if( this->i_mode & (Inode::IFCHR & Inode::IFBLK) )
	{
		return;
	}

	/* 采用FILO方式释放，以尽量使得SuperBlock中记录的空闲盘块号连续。
	 *
	 * Unix V6++的文件索引结构：(小型、大型和巨型文件)
	 * (1) i_addr[0] - i_addr[5]为直接索引表，文件长度范围是0 - 6个盘块；
	 *
	 * (2) i_addr[6] - i_addr[7]存放一次间接索引表所在磁盘块号，每磁盘块
	 * 上存放128个文件数据盘块号，此类文件长度范围是7 - (128 * 2 + 6)个盘块；
	 *
	 * (3) i_addr[8] - i_addr[9]存放二次间接索引表所在磁盘块号，每个二次间接
	 * 索引表记录128个一次间接索引表所在磁盘块号，此类文件长度范围是
	 * (128 * 2 + 6 ) < size <= (128 * 128 * 2 + 128 * 2 + 6)
	 */
	for(int i = 9; i >= 0; i--)		/* 从i_addr[9]到i_addr[0] */
	{
		/* 如果i_addr[]中第i项存在索引 */
		if( this->i_addr[i] != 0 )
		{
			/* 如果是i_addr[]中的一次间接、两次间接索引项 */
			if( i >= 6 && i <= 9 )
			{
				/* 将间接索引表读入缓存 */
				Buf* pFirstBuf = bm.Bread(this->i_dev, this->i_addr[i]);
				/* 获取缓冲区首址 */
				int* pFirst = (int *)pFirstBuf->b_addr;

				/* 每张间接索引表记录 512/sizeof(int) = 128个磁盘块号，遍历这全部128个磁盘块 */
				for(int j = 128 - 1; j >= 0; j--)
				{
					if( pFirst[j] != 0)	/* 如果该项存在索引 */
					{
						/*
						 * 如果是两次间接索引表，i_addr[8]或i_addr[9]项，
						 * 那么该字符块记录的是128个一次间接索引表存放的磁盘块号
						 */
						if( i >= 8 && i <= 9)
						{
							Buf* pSecondBuf = bm.Bread(this->i_dev, pFirst[j]);
							int* pSecond = (int *)pSecondBuf->b_addr;

							for(int k = 128 - 1; k >= 0; k--)
							{
								if(pSecond[k] != 0)
								{
									/* 释放指定的磁盘块 */
									filesys.Free(this->i_dev, pSecond[k]);
								}
							}
							/* 缓存使用完毕，释放以便被其它进程使用 */
							bm.Brelse(pSecondBuf);
						}
						filesys.Free(this->i_dev, pFirst[j]);
					}
				}
				bm.Brelse(pFirstBuf);
			}
			/* 释放索引表本身占用的磁盘块 */
			filesys.Free(this->i_dev, this->i_addr[i]);
			/* 0表示该项不包含索引 */
			this->i_addr[i] = 0;
		}
	}

	/* 盘块释放完毕，文件大小清零 */
	this->i_size = 0;
	/* 增设IUPD标志位，表示此内存Inode需要同步到相应外存Inode */
	this->i_flag |= Inode::IUPD;
	/* 清大文件标志 和原来的RWXRWXRWX比特*/
	this->i_mode &= ~(Inode::ILARG & Inode::IRWXU & Inode::IRWXG & Inode::IRWXO);
	this->i_nlink = 1;
}

/*============================class DiskInode=================================*/

DiskInode::DiskInode()
{
	/*
	 * 如果DiskInode没有构造函数，会发生如下较难察觉的错误：
	 * DiskInode作为局部变量占据函数Stack Frame中的内存空间，但是
	 * 这段空间没有被正确初始化，仍旧保留着先前栈内容，由于并不是
	 * DiskInode所有字段都会被更新，将DiskInode写回到磁盘上时，可能
	 * 将先前栈内容一同写回，导致写回结果出现莫名其妙的数据。
	 */
	this->d_mode = 0;
	this->d_nlink = 0;
	this->d_uid = -1;
	this->d_gid = -1;
	this->d_size = 0;
	for(int i = 0; i < 10; i++)
	{
		this->d_addr[i] = 0;
	}
	this->d_atime = 0;
	this->d_mtime = 0;
}

DiskInode::~DiskInode()
{
	//nothing to do here
}

extern "C" void inode_read(Inode* pInode) {
	pInode->ReadI();
}
