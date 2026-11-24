class Widget
  def label
    "w"
  end
end

class Order
  def widget
    Widget.new
  end
end
