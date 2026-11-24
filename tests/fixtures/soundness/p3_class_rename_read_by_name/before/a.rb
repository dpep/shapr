class Widget
  def label = self.class.name
end

puts Widget.new.label
