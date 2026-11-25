class A
  def self.sig(&blk)
    (@sigs ||= []) << blk
  end

  def total
    1
  end
end
