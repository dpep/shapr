class Item
  def to_h
    { size: round(size, places: 2) }
  end
end
