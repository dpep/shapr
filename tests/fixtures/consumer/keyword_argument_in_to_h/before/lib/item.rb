class Item
  def to_h
    { size: round(size, digits: 2) }
  end
end
