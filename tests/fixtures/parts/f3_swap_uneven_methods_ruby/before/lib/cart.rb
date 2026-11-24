class Cart
  def add(item)
    @items << item
  end

  def empty?
    (@items.empty?)
  end

  def count
    @items.size
  end

  private

  def check(item)
    raise ArgumentError unless item
  end
end
